#import "macos_bridge.h"

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>

#include <math.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>

static char *dup_json(NSDictionary *obj) {
  NSError *err = nil;
  NSData *data = [NSJSONSerialization dataWithJSONObject:obj
                                                 options:0
                                                   error:&err];
  if (!data) {
    return strdup("{\"ok\":false,\"code\":\"json_encode_error\"}");
  }
  char *buf = (char *)malloc(data.length + 1);
  if (!buf)
    return strdup("{\"ok\":false,\"code\":\"oom\"}");
  memcpy(buf, data.bytes, data.length);
  buf[data.length] = '\0';
  return buf;
}

void apexuse_free_cstr(char *ptr) {
  if (ptr)
    free(ptr);
}

static void ensure_cgs_init(void) {
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    // Headless CLI binaries must initialize NSApplication/WindowServer once
    // before calling ScreenCaptureKit (prevents CGS_REQUIRE_INIT abort).
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
    (void)CGMainDisplayID();
  });
}

// MARK: - Independent Agent Cursor Overlay (never moves user's hardware cursor)

@interface ApexUseCursorView : NSView
@property(nonatomic, assign) double rippleProgress;
@end

static double ease_out_cubic(double t) {
  double u = 1.0 - MAX(0.0, MIN(1.0, t));
  return 1.0 - (u * u * u);
}

@implementation ApexUseCursorView
- (void)drawRect:(NSRect)dirtyRect {
  [[NSColor clearColor] set];
  NSRectFill(dirtyRect);

  NSGraphicsContext *ctx = [NSGraphicsContext currentContext];
  [ctx saveGraphicsState];

  const CGFloat cx = 44.0;
  const CGFloat cy = 44.0;
  double p = self.rippleProgress;

  // Expanding click ripple centered right at the pointer hotspot
  if (p > 0.0 && p <= 1.0) {
    double e = ease_out_cubic(p);
    CGFloat r = 5.0 + 27.0 * e;
    NSRect outerRect = NSMakeRect(cx - r, cy - r, r * 2.0, r * 2.0);
    NSBezierPath *ripple = [NSBezierPath bezierPathWithOvalInRect:outerRect];
    [[NSColor colorWithCalibratedRed:0.05
                               green:0.65
                                blue:0.91
                               alpha:0.22 * (1.0 - p)] setFill];
    [ripple fill];
    [[NSColor colorWithCalibratedRed:0.08
                               green:0.78
                                blue:0.72
                               alpha:0.88 * pow(1.0 - p, 1.25)] setStroke];
    ripple.lineWidth = MAX(0.8, 2.2 - 1.1 * p);
    [ripple stroke];

    CGFloat dotR = MAX(0.5, 4.0 * (1.0 - p));
    NSRect dotRect = NSMakeRect(cx - dotR, cy - dotR, dotR * 2.0, dotR * 2.0);
    [[NSColor colorWithCalibratedRed:0.22
                               green:0.89
                                blue:0.84
                               alpha:0.9 * (1.0 - p)] setFill];
    [[NSBezierPath bezierPathWithOvalInRect:dotRect] fill];
  } else {
    // Subtle ambient tip halo so the agent hotspot reads cleanly on light/dark UIs
    NSRect haloRect = NSMakeRect(cx - 6.0, cy - 6.0, 12.0, 12.0);
    [[NSColor colorWithCalibratedRed:0.06
                               green:0.72
                                blue:0.86
                               alpha:0.14] setFill];
    [[NSBezierPath bezierPathWithOvalInRect:haloRect] fill];
  }

  // Subtle physical press scale anchored at tip (cx, cy) during clicks
  if (p > 0.0 && p < 1.0) {
    CGFloat s = 1.0 - 0.13 * sin(p * M_PI);
    NSAffineTransform *xf = [NSAffineTransform transform];
    [xf translateXBy:cx yBy:cy];
    [xf scaleBy:s];
    [xf translateXBy:-cx yBy:-cy];
    [xf concat];
  }

  // Classic macOS-proportioned slanted pointer with tip anchored cleanly at (cx, cy)
  NSBezierPath *arrow = [NSBezierPath bezierPath];
  [arrow moveToPoint:NSMakePoint(cx, cy)];
  [arrow lineToPoint:NSMakePoint(cx, cy - 21.0)];
  [arrow lineToPoint:NSMakePoint(cx + 5.2, cy - 16.2)];
  [arrow lineToPoint:NSMakePoint(cx + 9.1, cy - 24.2)];
  [arrow lineToPoint:NSMakePoint(cx + 12.2, cy - 22.7)];
  [arrow lineToPoint:NSMakePoint(cx + 8.4, cy - 14.7)];
  [arrow lineToPoint:NSMakePoint(cx + 15.2, cy - 14.6)];
  [arrow closePath];
  arrow.lineJoinStyle = NSLineJoinStyleRound;
  arrow.lineCapStyle = NSLineCapStyleRound;

  // Outer crisp white border + soft drop shadow
  [ctx saveGraphicsState];
  NSShadow *shadow = [[NSShadow alloc] init];
  shadow.shadowColor = [NSColor colorWithCalibratedWhite:0.0 alpha:0.38];
  shadow.shadowOffset = NSMakeSize(0.0, -2.5);
  shadow.shadowBlurRadius = 5.5;
  [shadow set];
  [[NSColor colorWithCalibratedWhite:1.0 alpha:0.98] setStroke];
  arrow.lineWidth = 3.2;
  [arrow stroke];
  [ctx restoreGraphicsState];

  // Smooth vibrant sky-cyan -> emerald-teal gradient fill inside the pointer
  [ctx saveGraphicsState];
  [arrow addClip];
  NSGradient *grad = [[NSGradient alloc]
      initWithStartingColor:[NSColor colorWithCalibratedRed:0.05
                                                      green:0.65
                                                       blue:0.92
                                                      alpha:1.0]
                endingColor:[NSColor colorWithCalibratedRed:0.06
                                                      green:0.78
                                                       blue:0.58
                                                      alpha:1.0]];
  [grad drawInRect:arrow.bounds angle:-50.0];
  [ctx restoreGraphicsState];

  [ctx restoreGraphicsState];
}
@end

static NSWindow *gCursorWin = nil;
static ApexUseCursorView *gCursorView = nil;
static CGPoint gLastAgentPt = {0, 0};
static BOOL gHaveLastAgentPt = NO;
static CFAbsoluteTime gCursorExpiresAt = 0;

static void pump_ui_ms(int ms) {
  CFRunLoopRunInMode(kCFRunLoopDefaultMode, (double)ms / 1000.0, false);
}

static void touch_agent_cursor_ttl(void) {
  gCursorExpiresAt = CFAbsoluteTimeGetCurrent() + 10.0;
}

static void show_agent_cursor_at(CGPoint cgPt, BOOL isClick) {
  if (![NSThread isMainThread])
    return;
  ensure_cgs_init();
  touch_agent_cursor_ttl();
  CGFloat mainH = NSScreen.screens.firstObject
                      ? NSScreen.screens.firstObject.frame.size.height
                      : (CGFloat)CGDisplayPixelsHigh(CGMainDisplayID());
  if (!gCursorWin) {
    gCursorWin =
        [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 88, 88)
                                    styleMask:NSWindowStyleMaskBorderless
                                      backing:NSBackingStoreBuffered
                                        defer:NO];
    gCursorWin.opaque = NO;
    gCursorWin.backgroundColor = [NSColor clearColor];
    gCursorWin.ignoresMouseEvents = YES;
    gCursorWin.hasShadow = NO;
    gCursorWin.alphaValue = 0.0;
    gCursorWin.level = NSScreenSaverWindowLevel;
    gCursorWin.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                    NSWindowCollectionBehaviorTransient;
    gCursorView =
        [[ApexUseCursorView alloc] initWithFrame:NSMakeRect(0, 0, 88, 88)];
    gCursorWin.contentView = gCursorView;
  }

  BOOL wasHidden = !gCursorWin.isVisible || gCursorWin.alphaValue < 0.05;
  CGPoint startPt = gHaveLastAgentPt ? gLastAgentPt : cgPt;
  double dist = hypot(cgPt.x - startPt.x, cgPt.y - startPt.y);
  int glideSteps = (!gHaveLastAgentPt || dist < 3.0)
                       ? (wasHidden ? 4 : 1)
                       : MAX(10, MIN(22, (int)round(dist / 36.0) + 8));

  gCursorView.rippleProgress = 0.0;
  [gCursorWin orderFrontRegardless];

  for (int i = 1; i <= glideSteps; i++) {
    double t = (double)i / (double)glideSteps;
    double e = ease_out_cubic(t);
    CGFloat x = startPt.x + (cgPt.x - startPt.x) * e;
    CGFloat y = startPt.y + (cgPt.y - startPt.y) * e;
    NSPoint origin = NSMakePoint(x - 44.0, mainH - y - 44.0);
    [gCursorWin setFrameOrigin:origin];
    if (wasHidden) {
      gCursorWin.alphaValue = MIN(1.0, (double)i / (double)MIN(4, glideSteps));
    } else if (gCursorWin.alphaValue < 1.0) {
      gCursorWin.alphaValue = 1.0;
    }
    [gCursorView setNeedsDisplay:YES];
    [gCursorView displayIfNeeded];
    pump_ui_ms(8);
  }
  gCursorWin.alphaValue = 1.0;
  gLastAgentPt = cgPt;
  gHaveLastAgentPt = YES;

  if (isClick) {
    const int rippleFrames = 11;
    for (int f = 1; f <= rippleFrames; f++) {
      gCursorView.rippleProgress = (double)f / (double)rippleFrames;
      [gCursorView setNeedsDisplay:YES];
      [gCursorView displayIfNeeded];
      pump_ui_ms(10);
    }
    gCursorView.rippleProgress = 0.0;
    [gCursorView setNeedsDisplay:YES];
    [gCursorView displayIfNeeded];
  }
  touch_agent_cursor_ttl();
}

static void hide_agent_cursor(void) {
  gCursorExpiresAt = 0;
  if (![NSThread isMainThread] || !gCursorWin)
    return;
  if (gCursorWin.isVisible && gCursorWin.alphaValue > 0.05) {
    for (int i = 4; i >= 0; i--) {
      gCursorWin.alphaValue = (CGFloat)i / 5.0;
      pump_ui_ms(12);
    }
  }
  [gCursorWin orderOut:nil];
  gCursorWin.alphaValue = 0.0;
  pump_ui_ms(10);
}

void apexuse_macos_tick(void) {
  if (![NSThread isMainThread])
    return;
  pump_ui_ms(15);
  if (gCursorWin && gCursorWin.isVisible && gCursorExpiresAt > 0 &&
      CFAbsoluteTimeGetCurrent() >= gCursorExpiresAt) {
    hide_agent_cursor();
  }
}

static BOOL is_trusted_system_app(NSRunningApplication *app) {
  NSString *bundle = app.bundleIdentifier.lowercaseString;
  if (!bundle)
    return NO;
  NSDictionary<NSString *, NSString *> *pinned = @{
    @"com.apple.textedit" : @"/System/Applications/TextEdit.app",
    @"com.apple.calculator" : @"/System/Applications/Calculator.app",
  };
  NSString *expected = pinned[bundle];
  if (!expected)
    return YES;
  NSString *actual = app.bundleURL.URLByResolvingSymlinksInPath.path;
  return [actual isEqualToString:expected];
}

static NSRunningApplication *resolve_app(NSString *bundleId) {
  if (!bundleId || bundleId.length == 0)
    return nil;
  NSString *target = bundleId.lowercaseString;
  NSMutableArray<NSRunningApplication *> *matches = [NSMutableArray array];
  for (NSRunningApplication *app in [NSWorkspace sharedWorkspace]
           .runningApplications) {
    if (app.bundleIdentifier &&
        [app.bundleIdentifier.lowercaseString isEqualToString:target]) {
      if (is_trusted_system_app(app)) {
        [matches addObject:app];
      }
    }
  }
  if (matches.count == 1)
    return matches.firstObject;
  for (NSRunningApplication *app in matches) {
    if (app.activationPolicy == NSApplicationActivationPolicyRegular &&
        app.isActive)
      return app;
  }
  for (NSRunningApplication *app in matches) {
    if (app.activationPolicy == NSApplicationActivationPolicyRegular)
      return app;
  }
  return matches.firstObject;
}

static id ax_copy(AXUIElementRef el, CFStringRef attr) {
  CFTypeRef val = NULL;
  if (AXUIElementCopyAttributeValue(el, attr, &val) != kAXErrorSuccess ||
      !val) {
    return nil;
  }
  return CFBridgingRelease(val);
}

static NSString *ax_string(AXUIElementRef el, CFStringRef attr) {
  id v = ax_copy(el, attr);
  return [v isKindOfClass:[NSString class]] ? (NSString *)v : nil;
}

static NSNumber *ax_bool(AXUIElementRef el, CFStringRef attr) {
  id v = ax_copy(el, attr);
  return [v isKindOfClass:[NSNumber class]] ? (NSNumber *)v : nil;
}

static BOOL ax_frame(AXUIElementRef el, CGRect *outRect) {
  CFTypeRef posRef = NULL;
  CFTypeRef sizeRef = NULL;
  if (AXUIElementCopyAttributeValue(el, kAXPositionAttribute, &posRef) !=
          kAXErrorSuccess ||
      !posRef) {
    return NO;
  }
  if (AXUIElementCopyAttributeValue(el, kAXSizeAttribute, &sizeRef) !=
          kAXErrorSuccess ||
      !sizeRef) {
    CFRelease(posRef);
    return NO;
  }
  CGPoint pt = CGPointZero;
  CGSize sz = CGSizeZero;
  BOOL ok = AXValueGetValue((AXValueRef)posRef, kAXValueCGPointType, &pt) &&
            AXValueGetValue((AXValueRef)sizeRef, kAXValueCGSizeType, &sz);
  CFRelease(posRef);
  CFRelease(sizeRef);
  if (!ok || sz.width <= 0 || sz.height <= 0)
    return NO;
  *outRect = CGRectMake(pt.x, pt.y, sz.width, sz.height);
  return YES;
}

static NSArray<NSString *> *ax_actions(AXUIElementRef el) {
  CFArrayRef arr = NULL;
  if (AXUIElementCopyActionNames(el, &arr) != kAXErrorSuccess || !arr)
    return @[];
  NSArray *res = CFBridgingRelease(arr);
  NSMutableArray<NSString *> *out = [NSMutableArray array];
  for (id item in res) {
    if ([item isKindOfClass:[NSString class]])
      [out addObject:item];
  }
  return out;
}

static BOOL is_chromium_or_electron(NSString *bundleId) {
  if (!bundleId)
    return NO;
  NSString *b = bundleId.lowercaseString;
  return [b isEqualToString:@"com.google.chrome"] ||
         [b isEqualToString:@"company.thebrowser.dia"] ||
         [b hasPrefix:@"com.google.antigravity"] ||
         [b isEqualToString:@"com.microsoft.vscode"] ||
         [b isEqualToString:@"com.tinyspeck.slackmacgap"] ||
         [b isEqualToString:@"com.openai.codex"] ||
         [b containsString:@"electron"] || [b containsString:@"chromium"];
}

static NSString *classify_role(AXUIElementRef el, BOOL isChromium,
                               BOOL *outSecure, BOOL *outUnverified) {
  (void)isChromium;
  CFTypeRef roleRef = NULL;
  CFTypeRef subroleRef = NULL;
  AXError rErr = AXUIElementCopyAttributeValue(el, kAXRoleAttribute, &roleRef);
  AXError sErr =
      AXUIElementCopyAttributeValue(el, kAXSubroleAttribute, &subroleRef);
  NSString *role = (rErr == kAXErrorSuccess &&
                    [(__bridge id)roleRef isKindOfClass:[NSString class]])
                       ? (__bridge NSString *)roleRef
                       : nil;
  NSString *subrole = (sErr == kAXErrorSuccess &&
                       [(__bridge id)subroleRef isKindOfClass:[NSString class]])
                          ? (__bridge NSString *)subroleRef
                          : nil;

  NSString *effective = role ?: @"AXUnknown";
  BOOL sec = [effective isEqualToString:@"AXSecureTextField"] ||
             [subrole isEqualToString:@"AXSecureTextField"];
  // Only mark as unverified when AX attribute lookup itself fails unexpectedly
  // (e.g. IPC failure), not when native/web fields simply omit AXSubrole.
  BOOL unv = !sec && ((rErr != kAXErrorSuccess && rErr != kAXErrorNoValue &&
                       rErr != kAXErrorAttributeUnsupported) ||
                      ([effective isEqualToString:@"AXTextField"] &&
                       sErr != kAXErrorSuccess && sErr != kAXErrorNoValue &&
                       sErr != kAXErrorAttributeUnsupported));

  if (roleRef)
    CFRelease(roleRef);
  if (subroleRef)
    CFRelease(subroleRef);

  if (sec) {
    *outSecure = YES;
    *outUnverified = NO;
    return @"AXSecureTextField";
  }
  if (unv) {
    *outSecure = NO;
    *outUnverified = YES;
    return @"AXUnverifiedTextField";
  }
  *outSecure = NO;
  *outUnverified = NO;
  return effective;
}

static NSString *truncate_str(NSString *s, NSUInteger maxLen) {
  if (!s)
    return nil;
  NSString *clean = [[s stringByReplacingOccurrencesOfString:@"\r"
                                                  withString:@" "]
      stringByReplacingOccurrencesOfString:@"\n"
                                withString:@" "];
  if (clean.length <= maxLen)
    return clean;
  return [[clean substringToIndex:maxLen] stringByAppendingString:@"..."];
}

static BOOL is_interactive_role(NSString *role) {
  static NSSet<NSString *> *roles = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    roles = [NSSet setWithArray:@[
      @"AXButton", @"AXTextField", @"AXSecureTextField",
      @"AXUnverifiedTextField", @"AXTextArea", @"AXSearchField", @"AXCheckBox",
      @"AXRadioButton", @"AXPopUpButton", @"AXComboBox", @"AXMenuItem",
      @"AXMenuButton", @"AXLink", @"AXSlider", @"AXSegmentedControl",
      @"AXDisclosureTriangle", @"AXTabGroup", @"AXIncrementor", @"AXColorWell"
    ]];
  });
  return [roles containsObject:role];
}

static void walk_ax_node(AXUIElementRef el, int depth, int maxDepth,
                         int maxElements, int maxSafetyDepth,
                         int maxSafetyNodes, CFAbsoluteTime deadline,
                         CGRect windowClip, int winIdx, BOOL isChromium,
                         NSMutableArray *elements, BOOL *hasSecure,
                         BOOL *hasUnverified, BOOL *truncated,
                         BOOL *depthTruncated, BOOL *safetyComplete,
                         int *scannedNodes, int *refCounter) {
  if (*scannedNodes >= maxSafetyNodes || depth > maxSafetyDepth ||
      CFAbsoluteTimeGetCurrent() > deadline) {
    if (depth > maxSafetyDepth)
      *depthTruncated = YES;
    else
      *truncated = YES;
    return;
  }
  (*scannedNodes)++;

  BOOL sec = NO;
  BOOL unv = NO;
  NSString *role = classify_role(el, isChromium, &sec, &unv);
  if (sec) {
    *hasSecure = YES;
    if ((int)elements.count < maxElements) {
      [elements addObject:@{
        @"role" : @"SecureTextField",
        @"window" : @(winIdx),
        @"depth" : @(depth),
        @"secure" : @YES,
      }];
    }
    return;
  }
  if (unv) {
    *hasUnverified = YES;
    if ((int)elements.count < maxElements) {
      [elements addObject:@{
        @"role" : @"UnverifiedTextField",
        @"window" : @(winIdx),
        @"depth" : @(depth),
        @"unverified" : @YES,
      }];
    }
    return;
  }

  CGRect frame = CGRectZero;
  BOOL hasFrame = ax_frame(el, &frame);
  // Prune off-screen / zero-size subtrees inside scrollable windows so
  // Electron/Chrome don't burn IPC time walking thousands of invisible lines,
  // while still scanning visible ones.
  if (depth >= 2 && hasFrame && !CGRectIsEmpty(windowClip)) {
    if (frame.size.width <= 2.0 || frame.size.height <= 2.0 ||
        !CGRectIntersectsRect(windowClip, frame)) {
      return;
    }
  }

  BOOL collectDisplay =
      ((int)elements.count < maxElements) && (depth <= maxDepth);
  if ((int)elements.count >= maxElements)
    *truncated = YES;
  if (depth > maxDepth)
    *depthTruncated = YES;

  if (collectDisplay) {
    NSString *title = truncate_str(ax_string(el, kAXTitleAttribute), 120);
    NSString *desc = truncate_str(ax_string(el, kAXDescriptionAttribute), 120);
    NSString *fname = truncate_str(ax_string(el, kAXFilenameAttribute), 120);
    NSString *label =
        (title.length > 0)
            ? title
            : ((desc.length > 0) ? desc : ((fname.length > 0) ? fname : @""));
    NSString *valStr = nil;
    if (![role isEqualToString:@"AXWindow"] &&
        ![role isEqualToString:@"AXApplication"]) {
      id rawVal = ax_copy(el, kAXValueAttribute);
      if ([rawVal isKindOfClass:[NSString class]]) {
        valStr = truncate_str((NSString *)rawVal, 160);
      } else if ([rawVal isKindOfClass:[NSNumber class]]) {
        valStr = [(NSNumber *)rawVal stringValue];
      }
    }
    NSNumber *enabled = ax_bool(el, kAXEnabledAttribute);
    NSNumber *focused = ax_bool(el, kAXFocusedAttribute);
    NSArray<NSString *> *actions = ax_actions(el);

    NSString *trimLabel = [label
        stringByTrimmingCharactersInSet:[NSCharacterSet
                                            whitespaceAndNewlineCharacterSet]];
    NSString *trimVal =
        valStr ? [valStr stringByTrimmingCharactersInSet:
                             [NSCharacterSet whitespaceAndNewlineCharacterSet]]
               : @"";
    BOOL bigUnlabeledGroup =
        ([role isEqualToString:@"AXGroup"] && trimLabel.length == 0 &&
         trimVal.length == 0 && hasFrame && !CGRectIsEmpty(windowClip) &&
         (frame.size.width * frame.size.height) >
             0.5 * (windowClip.size.width * windowClip.size.height));
    BOOL strongControl = is_interactive_role(role) ||
                         ([actions containsObject:@"AXPress"] &&
                          ![role isEqualToString:@"AXGroup"]) ||
                         [actions containsObject:@"AXOpen"];
    BOOL interactive =
        !bigUnlabeledGroup &&
        (is_interactive_role(role) || [actions containsObject:@"AXPress"] ||
         [actions containsObject:@"AXOpen"]);
    BOOL interestingText = ([role isEqualToString:@"AXStaticText"] &&
                            (trimLabel.length > 0 || trimVal.length > 0));
    BOOL isHeading =
        ([role isEqualToString:@"AXHeading"] && trimLabel.length > 0);
    BOOL isSheet = [role isEqualToString:@"AXSheet"];
    BOOL isGroupWithLabel = ([role isEqualToString:@"AXGroup"] &&
                             trimLabel.length > 0 && depth <= 9);
    int softTextCap = MAX(1, maxElements - (maxElements / 3));
    BOOL allowByPriority = strongControl || ((int)elements.count < softTextCap);

    if (allowByPriority && (interactive || interestingText || isHeading ||
                            isSheet || isGroupWithLabel)) {
      (*refCounter)++;
      NSString *refId = [NSString stringWithFormat:@"e%d", *refCounter];
      NSString *shortRole =
          [role hasPrefix:@"AX"] ? [role substringFromIndex:2] : role;

      NSMutableDictionary *entry = [NSMutableDictionary dictionary];
      entry[@"ref"] = refId;
      entry[@"role"] = shortRole;
      entry[@"ax_role"] = role;
      entry[@"window"] = @(winIdx);
      entry[@"depth"] = @(depth);
      entry[@"interactive"] = @(interactive);
      if (label.length > 0)
        entry[@"label"] = label;
      if (valStr.length > 0)
        entry[@"value"] = valStr;
      if (enabled)
        entry[@"enabled"] = enabled;
      if (focused && focused.boolValue)
        entry[@"focused"] = @YES;
      if (actions.count > 0)
        entry[@"actions"] = actions;
      if (hasFrame) {
        entry[@"frame"] = @{
          @"x" : @(round(frame.origin.x * 10.0) / 10.0),
          @"y" : @(round(frame.origin.y * 10.0) / 10.0),
          @"w" : @(round(frame.size.width * 10.0) / 10.0),
          @"h" : @(round(frame.size.height * 10.0) / 10.0),
        };
      }
      [elements addObject:entry];
    }
  }

  // Do not walk internal children of TextArea/StaticText/Image;
  // avoids thousands of redundant IPC calls on web/editor trees.
  if ([role isEqualToString:@"AXTextArea"] ||
      [role isEqualToString:@"AXStaticText"] ||
      [role isEqualToString:@"AXImage"])
    return;

  id childrenObj = ax_copy(el, kAXChildrenAttribute);
  if ([childrenObj isKindOfClass:[NSArray class]]) {
    NSArray *children = (NSArray *)childrenObj;
    // At window root, walk any modal AXSheet first so open/save dialogs always fit
    // inside maxElements and receive Set-of-Mark targets ahead of background web DOM.
    if (depth == 0 && children.count > 1) {
      NSMutableArray *sheets = [NSMutableArray array];
      NSMutableArray *others = [NSMutableArray arrayWithCapacity:children.count];
      for (id child in children) {
        NSString *cr = ax_string((__bridge AXUIElementRef)child, kAXRoleAttribute);
        if ([cr isEqualToString:@"AXSheet"])
          [sheets addObject:child];
        else
          [others addObject:child];
      }
      if (sheets.count > 0) {
        [sheets addObjectsFromArray:others];
        children = sheets;
      }
    }
    NSUInteger limit = MIN(children.count, (NSUInteger)500);
    if (children.count > limit)
      *truncated = YES;
    for (NSUInteger idx = 0; idx < limit; idx++) {
      if (*scannedNodes >= maxSafetyNodes ||
          CFAbsoluteTimeGetCurrent() > deadline) {
        *truncated = YES;
        break;
      }
      walk_ax_node((__bridge AXUIElementRef)children[idx], depth + 1, maxDepth,
                   maxElements, maxSafetyDepth, maxSafetyNodes, deadline,
                   windowClip, winIdx, isChromium, elements, hasSecure,
                   hasUnverified, truncated, depthTruncated, safetyComplete,
                   scannedNodes, refCounter);
    }
  }
}

char *apexuse_macos_doctor(void) {
  BOOL ax = AXIsProcessTrusted();
  BOOL sr = NO;
  if (@available(macOS 10.15, *)) {
    sr = CGPreflightScreenCaptureAccess();
  }
  return dup_json(@{
    @"ok" : @YES,
    @"os" : @"macos",
    @"accessibility" : @(ax),
    @"screen_recording" : @(sr),
  });
}

char *apexuse_macos_list_apps(void) {
  NSMutableArray *apps = [NSMutableArray array];
  for (NSRunningApplication *app in [NSWorkspace sharedWorkspace]
           .runningApplications) {
    if (app.activationPolicy != NSApplicationActivationPolicyRegular)
      continue;
    if (!is_trusted_system_app(app))
      continue;
    if (!app.bundleIdentifier)
      continue;
    [apps addObject:@{
      @"bundle_id" : app.bundleIdentifier,
      @"pid" : @(app.processIdentifier),
      @"active" : @(app.isActive),
    }];
  }
  return dup_json(@{@"ok" : @YES, @"apps" : apps});
}

char *apexuse_macos_activate_app(const char *bundle_id) {
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  NSRunningApplication *app = resolve_app(bid);
  if (!app)
    return dup_json(@{@"ok" : @NO, @"code" : @"app_not_running"});
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  BOOL ok = [app activateWithOptions:NSApplicationActivateIgnoringOtherApps];
#pragma clang diagnostic pop
  return dup_json(@{
    @"ok" : @(ok),
    @"pid" : @(app.processIdentifier),
    @"bundle_id" : app.bundleIdentifier ?: bid
  });
}

char *apexuse_macos_launch_app(const char *bundle_id) {
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  NSTask *task = [[NSTask alloc] init];
  task.executableURL = [NSURL fileURLWithPath:@"/usr/bin/open"];
  task.arguments = @[ @"-b", bid ];
  NSError *err = nil;
  if (![task launchAndReturnError:&err]) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"launch_failed",
      @"message" : err.localizedDescription ?: @"open failed"
    });
  }
  [task waitUntilExit];
  if (task.terminationStatus != 0) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"launch_failed",
      @"message" : @"open exited nonzero"
    });
  }
  return dup_json(@{@"ok" : @YES, @"bundle_id" : bid});
}

char *apexuse_macos_window_state(const char *bundle_id) {
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  BOOL isDesktop = [bid.lowercaseString isEqualToString:@"desktop"];
  NSRunningApplication *frontApp =
      [NSWorkspace sharedWorkspace].frontmostApplication;
  NSString *frontBundle = frontApp.bundleIdentifier ?: @"";
  NSRunningApplication *app = isDesktop ? frontApp : resolve_app(bid);
  if (!isDesktop && !app)
    return dup_json(@{@"ok" : @NO, @"code" : @"app_not_running"});
  pid_t pid = app ? app.processIdentifier : 0;

  size_t totalWindows = 0;
  CGRect activeFrame = CGRectZero;
  BOOL haveActiveFrame = NO;

  if (!isDesktop && AXIsProcessTrusted()) {
    AXUIElementRef appEl = AXUIElementCreateApplication(pid);
    id winsObj = ax_copy(appEl, kAXWindowsAttribute);
    if ([winsObj isKindOfClass:[NSArray class]]) {
      NSArray *axWins = (NSArray *)winsObj;
      totalWindows = axWins.count;
      CFTypeRef focusedWin = NULL;
      if (AXUIElementCopyAttributeValue(appEl, kAXFocusedWindowAttribute,
                                        &focusedWin) == kAXErrorSuccess &&
          focusedWin) {
        CGRect f = CGRectZero;
        if (ax_frame((AXUIElementRef)focusedWin, &f) && f.size.width > 10 &&
            f.size.height > 10) {
          haveActiveFrame = YES;
          activeFrame = f;
        }
        CFRelease(focusedWin);
      }
      if (!haveActiveFrame && axWins.count > 0) {
        for (id w in axWins) {
          CGRect f = CGRectZero;
          if (ax_frame((__bridge AXUIElementRef)w, &f) && f.size.width > 50 &&
              f.size.height > 50) {
            haveActiveFrame = YES;
            activeFrame = f;
            break;
          }
        }
      }
    }
    CFRelease(appEl);
  }

  if (totalWindows == 0 || !haveActiveFrame) {
    CFArrayRef winList = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID);
    if (winList) {
      NSArray *arr = (__bridge NSArray *)winList;
      size_t count = 0;
      for (NSDictionary *info in arr) {
        pid_t winPid = (pid_t)[info[(id)kCGWindowOwnerPID] intValue];
        int layer = [info[(id)kCGWindowLayer] intValue];
        if (isDesktop || (winPid == pid && layer == 0)) {
          count++;
          if (!haveActiveFrame) {
            CGRect f = CGRectZero;
            if (CGRectMakeWithDictionaryRepresentation(
                    (__bridge CFDictionaryRef)info[(id)kCGWindowBounds], &f) &&
                f.size.width > 50 && f.size.height > 50) {
              haveActiveFrame = YES;
              activeFrame = f;
            }
          }
        }
      }
      if (totalWindows == 0)
        totalWindows = count;
      CFRelease(winList);
    }
  }

  if (isDesktop && !haveActiveFrame) {
    CGDirectDisplayID mainId = CGMainDisplayID();
    activeFrame = CGDisplayBounds(mainId);
    haveActiveFrame = YES;
  }

  NSMutableDictionary *res = [NSMutableDictionary dictionaryWithDictionary:@{
    @"ok" : @YES,
    @"frontmost_bundle_id" : frontBundle,
    @"pid" : @(pid),
    @"windows_total" : @(totalWindows),
  }];
  if (haveActiveFrame) {
    res[@"active_window_frame"] = @{
      @"x" : @(activeFrame.origin.x),
      @"y" : @(activeFrame.origin.y),
      @"w" : @(activeFrame.size.width),
      @"h" : @(activeFrame.size.height),
    };
  }
  return dup_json(res);
}

char *apexuse_macos_inspect_app(const char *bundle_id, int window_index,
                                 int max_elements, int max_depth) {
  ensure_cgs_init();
  if ([NSThread isMainThread] && gCursorWin && gCursorWin.isVisible)
    touch_agent_cursor_ttl();
  if (!AXIsProcessTrusted()) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"accessibility_denied",
      @"message" : @"Grant macOS Accessibility permission."
    });
  }
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  BOOL isDesktop = [bid.lowercaseString isEqualToString:@"desktop"];
  NSRunningApplication *app =
      isDesktop ? [NSWorkspace sharedWorkspace].frontmostApplication
                : resolve_app(bid);
  if (!app) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"app_not_running",
      @"message" : @"Application is not running."
    });
  }

  pid_t pid = app.processIdentifier;
  AXUIElementRef appEl = AXUIElementCreateApplication(pid);
  BOOL isChromium = is_chromium_or_electron(app.bundleIdentifier);
  if (isChromium) {
    // Ask Chromium/Electron apps to populate their semantic accessibility tree.
    AXUIElementSetAttributeValue(appEl, CFSTR("AXManualAccessibility"),
                                 kCFBooleanTrue);
    AXUIElementSetAttributeValue(appEl, CFSTR("AXEnhancedUserInterface"),
                                 kCFBooleanTrue);
  }
  id winsObj = ax_copy(appEl, kAXWindowsAttribute);
  NSArray *windows =
      [winsObj isKindOfClass:[NSArray class]] ? (NSArray *)winsObj : @[];
  if (window_index >= 0 && window_index >= (int)windows.count) {
    CFRelease(appEl);
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"window_not_found",
      @"message" : @"Window index out of range."
    });
  }

  NSMutableArray *winMetas = [NSMutableArray array];
  NSMutableArray *elements = [NSMutableArray array];
  BOOL hasSecure = NO;
  BOOL hasUnverified = NO;
  BOOL truncated = NO;
  BOOL depthTruncated = NO;
  BOOL safetyComplete = YES;
  int scannedNodes = 0;
  int refCounter = 0;
  CFAbsoluteTime deadline = CFAbsoluteTimeGetCurrent() + 3.0;
  int effectiveMaxElements = isChromium ? MAX(max_elements, 450) : max_elements;
  int effectiveMaxDepth = isChromium ? MAX(max_depth, 64) : max_depth;
  int maxSafetyDepth = isChromium ? 80 : 40;
  int maxSafetyNodes = isChromium ? 5000 : 2500;

  for (int i = 0; i < (int)windows.count; i++) {
    AXUIElementRef winEl = (__bridge AXUIElementRef)windows[i];
    CGRect wf = CGRectZero;
    BOOL hasF = ax_frame(winEl, &wf);
    NSString *wTitle =
        truncate_str(ax_string(winEl, kAXTitleAttribute), 120) ?: @"";
    NSMutableDictionary *wm = [NSMutableDictionary dictionary];
    wm[@"index"] = @(i);
    wm[@"title"] = wTitle;
    if (hasF) {
      wm[@"frame"] = @{
        @"x" : @(wf.origin.x),
        @"y" : @(wf.origin.y),
        @"w" : @(wf.size.width),
        @"h" : @(wf.size.height)
      };
    }
    [winMetas addObject:wm];

    if (window_index >= 0 && i != window_index)
      continue;
    walk_ax_node(winEl, 0, effectiveMaxDepth, effectiveMaxElements,
                 maxSafetyDepth, maxSafetyNodes, deadline,
                 hasF ? wf : CGRectZero, i, isChromium, elements, &hasSecure,
                 &hasUnverified, &truncated, &depthTruncated, &safetyComplete,
                 &scannedNodes, &refCounter);
  }
  CFRelease(appEl);

  return dup_json(@{
    @"ok" : @YES,
    @"bundle_id" : isDesktop ? @"desktop" : (app.bundleIdentifier ?: bid),
    @"frontmost_bundle_id" : app.bundleIdentifier ?: @"",
    @"pid" : @(pid),
    @"frontmost" : @(app.isActive),
    @"windows_total" : @(windows.count),
    @"windows" : winMetas,
    @"elements" : elements,
    @"has_secure_field" : @(hasSecure),
    @"has_unverified_field" : @(hasUnverified),
    @"truncated" : @(truncated),
    @"depth_truncated" : @(depthTruncated),
    @"safety_scan_complete" : @(safetyComplete),
  });
}

static NSString *encode_image_b64(CGImageRef img, int format, int quality) {
  if (!img)
    return nil;
  BOOL isJpeg = (format == 1);
  CFStringRef uti = isJpeg ? CFSTR("public.jpeg") : CFSTR("public.png");
  NSMutableData *data = [NSMutableData data];
  CGImageDestinationRef dest = CGImageDestinationCreateWithData(
      (__bridge CFMutableDataRef)data, uti, 1, NULL);
  if (!dest)
    return nil;

  NSDictionary *props = nil;
  if (isJpeg) {
    float q = (float)MAX(1, MIN(100, quality)) / 100.0f;
    props = @{
      (__bridge NSString *)kCGImageDestinationLossyCompressionQuality : @(q),
    };
  }

  CGImageDestinationAddImage(dest, img, (__bridge CFDictionaryRef)props);
  BOOL ok = CGImageDestinationFinalize(dest);
  CFRelease(dest);
  if (!ok)
    return nil;
  return [data base64EncodedStringWithOptions:0];
}

static NSString *png_b64_from_cgimage(CGImageRef img) {
  return encode_image_b64(img, 0, 80);
}

typedef struct {
  char bundle_id[128];
  int window_index;
  BOOL is_desktop;
  pid_t pid;
  uint32_t wid;
  CGRect frame;
  double timestamp_ms;
} WindowCacheEntry;

#define MAX_WIN_CACHE 16
static WindowCacheEntry gWinCache[MAX_WIN_CACHE];
static size_t gWinCacheCount = 0;
static pthread_mutex_t gWinCacheMutex = PTHREAD_MUTEX_INITIALIZER;

static BOOL lookup_window_cache(const char *bundle_id, int window_index,
                                BOOL is_desktop, pid_t *outPid,
                                uint32_t *outWid, CGRect *outFrame) {
  if (!bundle_id)
    return NO;
  double now = CFAbsoluteTimeGetCurrent() * 1000.0;
  pthread_mutex_lock(&gWinCacheMutex);
  for (size_t i = 0; i < gWinCacheCount; i++) {
    if (gWinCache[i].is_desktop == is_desktop &&
        gWinCache[i].window_index == window_index &&
        strcmp(gWinCache[i].bundle_id, bundle_id) == 0) {
      if (now - gWinCache[i].timestamp_ms < 1500.0) {
        if (outPid)
          *outPid = gWinCache[i].pid;
        if (outWid)
          *outWid = gWinCache[i].wid;
        if (outFrame)
          *outFrame = gWinCache[i].frame;
        pthread_mutex_unlock(&gWinCacheMutex);
        return YES;
      }
      gWinCache[i] = gWinCache[gWinCacheCount - 1];
      gWinCacheCount--;
      break;
    }
  }
  pthread_mutex_unlock(&gWinCacheMutex);
  return NO;
}

static void store_window_cache(const char *bundle_id, int window_index,
                               BOOL is_desktop, pid_t pid, uint32_t wid,
                               CGRect frame) {
  if (!bundle_id)
    return;
  double now = CFAbsoluteTimeGetCurrent() * 1000.0;
  pthread_mutex_lock(&gWinCacheMutex);
  for (size_t i = 0; i < gWinCacheCount; i++) {
    if (gWinCache[i].is_desktop == is_desktop &&
        gWinCache[i].window_index == window_index &&
        strcmp(gWinCache[i].bundle_id, bundle_id) == 0) {
      gWinCache[i].pid = pid;
      gWinCache[i].wid = wid;
      gWinCache[i].frame = frame;
      gWinCache[i].timestamp_ms = now;
      pthread_mutex_unlock(&gWinCacheMutex);
      return;
    }
  }
  if (gWinCacheCount < MAX_WIN_CACHE) {
    strncpy(gWinCache[gWinCacheCount].bundle_id, bundle_id,
            sizeof(gWinCache[0].bundle_id) - 1);
    gWinCache[gWinCacheCount].bundle_id[sizeof(gWinCache[0].bundle_id) - 1] =
        '\0';
    gWinCache[gWinCacheCount].window_index = window_index;
    gWinCache[gWinCacheCount].is_desktop = is_desktop;
    gWinCache[gWinCacheCount].pid = pid;
    gWinCache[gWinCacheCount].wid = wid;
    gWinCache[gWinCacheCount].frame = frame;
    gWinCache[gWinCacheCount].timestamp_ms = now;
    gWinCacheCount++;
  } else {
    strncpy(gWinCache[0].bundle_id, bundle_id,
            sizeof(gWinCache[0].bundle_id) - 1);
    gWinCache[0].bundle_id[sizeof(gWinCache[0].bundle_id) - 1] = '\0';
    gWinCache[0].window_index = window_index;
    gWinCache[0].is_desktop = is_desktop;
    gWinCache[0].pid = pid;
    gWinCache[0].wid = wid;
    gWinCache[0].frame = frame;
    gWinCache[0].timestamp_ms = now;
  }
  pthread_mutex_unlock(&gWinCacheMutex);
}

static void invalidate_window_cache(const char *bundle_id, int window_index,
                                    BOOL is_desktop) {
  pthread_mutex_lock(&gWinCacheMutex);
  for (size_t i = 0; i < gWinCacheCount; i++) {
    if (gWinCache[i].is_desktop == is_desktop &&
        gWinCache[i].window_index == window_index &&
        (!bundle_id || strcmp(gWinCache[i].bundle_id, bundle_id) == 0)) {
      gWinCache[i] = gWinCache[gWinCacheCount - 1];
      gWinCacheCount--;
      break;
    }
  }
  pthread_mutex_unlock(&gWinCacheMutex);
}

static pthread_mutex_t gCapMutex = PTHREAD_MUTEX_INITIALIZER;
static CGImageRef gCachedCapImg = NULL;
static NSString *gCachedCapId = nil;
static uint64_t gCapSeq = 0;

static NSString *store_cached_capture(CGImageRef img) {
  if (!img)
    return nil;
  pthread_mutex_lock(&gCapMutex);
  if (gCachedCapImg) {
    CGImageRelease(gCachedCapImg);
    gCachedCapImg = NULL;
  }
  gCachedCapImg = CGImageRetain(img);
  gCapSeq++;
  gCachedCapId = [NSString stringWithFormat:@"cap_%llu", (unsigned long long)gCapSeq];
  NSString *cid = [gCachedCapId copy];
  pthread_mutex_unlock(&gCapMutex);
  return cid;
}

static CGImageRef take_cached_capture(NSString *capId) {
  if (!capId)
    return NULL;
  pthread_mutex_lock(&gCapMutex);
  CGImageRef out = NULL;
  if (gCachedCapImg && gCachedCapId && [gCachedCapId isEqualToString:capId]) {
    out = CGImageRetain(gCachedCapImg);
  }
  pthread_mutex_unlock(&gCapMutex);
  return out;
}

static NSString *compute_pixel_digest(CGImageRef img) {
  if (!img)
    return nil;
  const size_t grid = 16;
  uint8_t buf[16 * 16];
  memset(buf, 0, sizeof(buf));
  CGColorSpaceRef cs = CGColorSpaceCreateDeviceGray();
  if (!cs)
    return nil;
  CGContextRef ctx = CGBitmapContextCreate(
      buf, grid, grid, 8, grid, cs, (CGBitmapInfo)kCGImageAlphaNone);
  CGColorSpaceRelease(cs);
  if (!ctx)
    return nil;
  CGContextSetInterpolationQuality(ctx, kCGInterpolationLow);
  CGContextDrawImage(ctx, CGRectMake(0, 0, (CGFloat)grid, (CGFloat)grid), img);
  CGContextRelease(ctx);
  NSMutableString *hex = [NSMutableString stringWithCapacity:grid * grid * 2];
  for (size_t i = 0; i < grid * grid; i++) {
    [hex appendFormat:@"%02x", buf[i]];
  }
  return hex;
}

char *apexuse_macos_capture_and_ocr(const char *bundle_id, int window_index,
                                     int ocr_mode, int encode_image,
                                     int image_format, int quality,
                                     int max_width) {
  if ([NSThread isMainThread])
    ensure_cgs_init();
  BOOL restoreCursorAfterCapture = NO;
  if ([NSThread isMainThread] && gCursorWin && gCursorWin.isVisible) {
    touch_agent_cursor_ttl();
    if (bundle_id && strcasecmp(bundle_id, "desktop") == 0) {
      restoreCursorAfterCapture = YES;
      [gCursorWin orderOut:nil];
      pump_ui_ms(10);
    }
  }
  if (@available(macOS 10.15, *)) {
    if (!CGPreflightScreenCaptureAccess()) {
      return dup_json(@{
        @"ok" : @NO,
        @"code" : @"screen_recording_denied",
        @"message" : @"Grant macOS Screen Recording permission."
      });
    }
  }
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  BOOL isDesktop = [bid.lowercaseString isEqualToString:@"desktop"];
  NSRunningApplication *frontApp =
      [NSWorkspace sharedWorkspace].frontmostApplication;
  NSRunningApplication *app = isDesktop ? frontApp : resolve_app(bid);
  if (!isDesktop && !app)
    return dup_json(@{@"ok" : @NO, @"code" : @"app_not_running"});
  pid_t pid = app ? app.processIdentifier : 0;

  pid_t cachedPid = 0;
  __block uint32_t cachedWid = 0;
  CGRect cachedFrame = CGRectZero;
  __block BOOL haveCache = lookup_window_cache(bundle_id, window_index, isDesktop,
                                               &cachedPid, &cachedWid, &cachedFrame);

  CGRect targetAxFrame = CGRectZero;
  NSString *targetAxTitle = nil;
  BOOL haveAxFrame = NO;
  if (!isDesktop && !haveCache && AXIsProcessTrusted()) {
    AXUIElementRef appEl = AXUIElementCreateApplication(pid);
    id winsObj = ax_copy(appEl, kAXWindowsAttribute);
    NSArray *axWins =
        [winsObj isKindOfClass:[NSArray class]] ? (NSArray *)winsObj : @[];

    int bestStdIdx = -1;
    double bestStdScore = -1e18;
    double maxStdArea = 0.0;
    for (int wi = 0; wi < (int)axWins.count; wi++) {
      AXUIElementRef wEl = (__bridge AXUIElementRef)axWins[wi];
      CGRect wf = CGRectZero;
      if (!ax_frame(wEl, &wf) || wf.size.width <= 120 || wf.size.height <= 140)
        continue;
      NSString *sub = ax_string(wEl, kAXSubroleAttribute) ?: @"";
      BOOL isStd = [sub isEqualToString:@"AXStandardWindow"] ||
                   [sub isEqualToString:@"AXDialog"];
      double area = wf.size.width * wf.size.height;
      if (isStd && area > maxStdArea)
        maxStdArea = area;
    }
    for (int wi = 0; wi < (int)axWins.count; wi++) {
      AXUIElementRef wEl = (__bridge AXUIElementRef)axWins[wi];
      CGRect wf = CGRectZero;
      if (!ax_frame(wEl, &wf) || wf.size.width <= 120 || wf.size.height <= 140)
        continue;
      NSString *sub = ax_string(wEl, kAXSubroleAttribute) ?: @"";
      NSString *wt = ax_string(wEl, kAXTitleAttribute) ?: @"";
      BOOL isStd = [sub isEqualToString:@"AXStandardWindow"] ||
                   [sub isEqualToString:@"AXDialog"];
      double area = wf.size.width * wf.size.height;
      double sc = (isStd ? 1e12 : 0.0) + (wt.length > 0 ? 1e9 : 0.0) +
                  ((maxStdArea > 0 && area >= maxStdArea * 0.45)
                       ? (1e7 - wi * 1000.0)
                       : area);
      if (sc > bestStdScore) {
        bestStdScore = sc;
        bestStdIdx = wi;
      }
    }

    int axIdx = (window_index >= 0) ? window_index : (bestStdIdx >= 0 ? bestStdIdx : 0);
    if (window_index >= 0 && window_index < (int)axWins.count && bestStdIdx >= 0 &&
        bestStdIdx != window_index) {
      AXUIElementRef reqW = (__bridge AXUIElementRef)axWins[window_index];
      NSString *reqSub = ax_string(reqW, kAXSubroleAttribute) ?: @"";
      NSString *reqTitle = ax_string(reqW, kAXTitleAttribute) ?: @"";
      CGRect reqF = CGRectZero;
      BOOL reqIsStd = [reqSub isEqualToString:@"AXStandardWindow"] ||
                      [reqSub isEqualToString:@"AXDialog"];
      if (!reqIsStd && reqTitle.length == 0 && ax_frame(reqW, &reqF) &&
          maxStdArea > 0 && (reqF.size.width * reqF.size.height) < maxStdArea * 0.55) {
        axIdx = bestStdIdx;
      }
    }
    if (axIdx >= 0 && axIdx < (int)axWins.count) {
      AXUIElementRef targetWin = (__bridge AXUIElementRef)axWins[axIdx];
      haveAxFrame = ax_frame(targetWin, &targetAxFrame);
      targetAxTitle = ax_string(targetWin, kAXTitleAttribute);
    }
    CFRelease(appEl);
  }

  __block CGImageRef captured = NULL;
  __block CGRect winFrame = CGRectZero;
  __block uint32_t capturedWid = 0;
  __block NSString *errCode = nil;
  __block NSString *errMsg = nil;
  dispatch_semaphore_t sem = dispatch_semaphore_create(0);

  [SCShareableContent getShareableContentWithCompletionHandler:^(
                          SCShareableContent *content, NSError *err) {
    if (err || !content) {
      errCode = @"screen_recording_denied";
      errMsg =
          err.localizedDescription ?: @"Unable to enumerate shareable windows.";
      dispatch_semaphore_signal(sem);
      return;
    }
    if (isDesktop) {
      CGDirectDisplayID mainId = CGMainDisplayID();
      SCDisplay *disp = nil;
      for (SCDisplay *d in content.displays) {
        if (d.displayID == mainId) {
          disp = d;
          break;
        }
      }
      if (!disp)
        disp = content.displays.firstObject;
      if (!disp) {
        errCode = @"display_not_found";
        errMsg = @"No active display found.";
        dispatch_semaphore_signal(sem);
        return;
      }
      winFrame = disp.frame;
      capturedWid = disp.displayID;
      SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:disp
                                                        excludingWindows:@[]];
      SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
      if (max_width > 0 && disp.frame.size.width > (CGFloat)max_width) {
        double scale = (double)max_width / disp.frame.size.width;
        cfg.width = (size_t)MAX(1.0, round(disp.frame.size.width * scale));
        cfg.height = (size_t)MAX(1.0, round(disp.frame.size.height * scale));
      } else {
        cfg.width = (size_t)MAX(1.0, round(disp.frame.size.width));
        cfg.height = (size_t)MAX(1.0, round(disp.frame.size.height));
      }
      cfg.showsCursor = NO;
      [SCScreenshotManager
          captureImageWithFilter:filter
                   configuration:cfg
               completionHandler:^(CGImageRef image, NSError *capErr) {
                 if (image) {
                   captured = CGImageRetain(image);
                   store_window_cache(bundle_id, window_index, isDesktop, pid,
                                      capturedWid, winFrame);
                 } else {
                   if (haveCache)
                     invalidate_window_cache(bundle_id, window_index, isDesktop);
                   errCode = @"capture_failed";
                   errMsg = capErr.localizedDescription
                                ?: @"SCScreenshotManager failed.";
                 }
                 dispatch_semaphore_signal(sem);
               }];
      return;
    }

    SCWindow *chosen = nil;
    if (haveCache && cachedWid > 0) {
      for (SCWindow *w in content.windows) {
        if (w.windowID == cachedWid && w.frame.size.width > 10 &&
            w.frame.size.height > 10) {
          chosen = w;
          break;
        }
      }
      if (!chosen) {
        haveCache = NO;
        invalidate_window_cache(bundle_id, window_index, isDesktop);
      }
    }

    if (!chosen) {
      NSMutableArray<SCWindow *> *candidates = [NSMutableArray array];
      for (SCWindow *w in content.windows) {
        if (w.owningApplication.processID == pid && w.windowLayer == 0 &&
            w.frame.size.width > 120 && w.frame.size.height > 140) {
          [candidates addObject:w];
        }
      }
      if (candidates.count == 0) {
        for (SCWindow *w in content.windows) {
          if (w.owningApplication.processID == pid && w.windowLayer == 0 &&
              w.frame.size.width > 50 && w.frame.size.height > 60) {
            [candidates addObject:w];
          }
        }
      }
      if (candidates.count == 0) {
        for (SCWindow *w in content.windows) {
          if (w.owningApplication.processID == pid && w.windowLayer == 0 &&
              w.frame.size.width > 10 && w.frame.size.height > 10) {
            [candidates addObject:w];
          }
        }
      }
      if (candidates.count == 0) {
        errCode = @"window_not_found";
        errMsg = @"No capturable window found for target application.";
        dispatch_semaphore_signal(sem);
        return;
      }
      if (haveAxFrame || targetAxTitle.length > 0) {
        double bestScore = 1e18;
        for (SCWindow *w in candidates) {
          double dist =
              haveAxFrame
                  ? (fabs(w.frame.origin.x - targetAxFrame.origin.x) +
                     fabs(w.frame.origin.y - targetAxFrame.origin.y) +
                     fabs(w.frame.size.width - targetAxFrame.size.width) +
                     fabs(w.frame.size.height - targetAxFrame.size.height))
                  : 0.0;
          double titleBonus = (targetAxTitle.length > 0 && w.title &&
                               [w.title isEqualToString:targetAxTitle])
                                  ? -5000.0
                                  : 0.0;
          double onScreenBonus = w.isOnScreen ? -200.0 : 0.0;
          double areaPenalty =
              haveAxFrame ? 0.0
                          : -(w.frame.size.width * w.frame.size.height) * 1e-6;
          double score = dist + titleBonus + onScreenBonus + areaPenalty;
          if (score < bestScore) {
            bestScore = score;
            chosen = w;
          }
        }
      } else {
        double bestScore = 1e18;
        for (SCWindow *w in candidates) {
          double onScreenBonus = w.isOnScreen ? -1e12 : 0.0;
          double titleBonus = (w.title && w.title.length > 0) ? -1e7 : 0.0;
          double score = onScreenBonus + titleBonus -
                         (w.frame.size.width * w.frame.size.height);
          if (score < bestScore) {
            bestScore = score;
            chosen = w;
          }
        }
      }
    }

    winFrame = chosen.frame;
    capturedWid = chosen.windowID;
    SCContentFilter *filter =
        [[SCContentFilter alloc] initWithDesktopIndependentWindow:chosen];
    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    if (max_width > 0 && chosen.frame.size.width > (CGFloat)max_width) {
      double scale = (double)max_width / chosen.frame.size.width;
      cfg.width = (size_t)MAX(1.0, round(chosen.frame.size.width * scale));
      cfg.height = (size_t)MAX(1.0, round(chosen.frame.size.height * scale));
    } else {
      cfg.width = (size_t)MAX(1.0, round(chosen.frame.size.width));
      cfg.height = (size_t)MAX(1.0, round(chosen.frame.size.height));
    }
    cfg.showsCursor = NO;
    [SCScreenshotManager
        captureImageWithFilter:filter
                 configuration:cfg
             completionHandler:^(CGImageRef image, NSError *capErr) {
               if (image) {
                 captured = CGImageRetain(image);
                 store_window_cache(bundle_id, window_index, isDesktop, pid,
                                    capturedWid, winFrame);
               } else {
                 if (haveCache)
                   invalidate_window_cache(bundle_id, window_index, isDesktop);
                 errCode = @"capture_failed";
                 errMsg = capErr.localizedDescription
                              ?: @"SCScreenshotManager failed.";
               }
               dispatch_semaphore_signal(sem);
             }];
  }];

  dispatch_semaphore_wait(sem,
                          dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC));
  if (restoreCursorAfterCapture && gCursorWin) {
    [gCursorWin orderFrontRegardless];
    touch_agent_cursor_ttl();
    pump_ui_ms(5);
  }
  if (!captured) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : errCode ?: @"capture_timeout",
      @"message" : errMsg ?: @"Timed out capturing window.",
    });
  }

  NSString *capId = store_cached_capture(captured);
  NSString *pixelDigest = compute_pixel_digest(captured);
  NSString *b64 = encode_image
                      ? encode_image_b64(captured, image_format, quality)
                      : @"";
  NSMutableArray *ocrBoxes = [NSMutableArray array];

  if (ocr_mode > 0) {
    VNRecognizeTextRequest *req = [[VNRecognizeTextRequest alloc] init];
    req.recognitionLevel = (ocr_mode == 1)
                               ? VNRequestTextRecognitionLevelFast
                               : VNRequestTextRecognitionLevelAccurate;
    req.usesLanguageCorrection = NO;
    VNImageRequestHandler *handler =
        [[VNImageRequestHandler alloc] initWithCGImage:captured options:@{}];
    NSError *ocrErr = nil;
    if ([handler performRequests:@[ req ] error:&ocrErr]) {
      for (VNRecognizedTextObservation *obs in req.results) {
        VNRecognizedText *top = [[obs topCandidates:1] firstObject];
        if (!top || top.string.length == 0)
          continue;
        CGRect bb = obs.boundingBox;
        double nx = bb.origin.x;
        double ny = 1.0 - bb.origin.y - bb.size.height;
        double sx = winFrame.origin.x + nx * winFrame.size.width;
        double sy = winFrame.origin.y + ny * winFrame.size.height;
        double sw = bb.size.width * winFrame.size.width;
        double sh = bb.size.height * winFrame.size.height;
        if (sw < 4 || sh < 4)
          continue;
        [ocrBoxes addObject:@{
          @"text" : truncate_str(top.string, 100),
          @"confidence" : @(obs.confidence),
          @"x" : @(round(sx * 10.0) / 10.0),
          @"y" : @(round(sy * 10.0) / 10.0),
          @"w" : @(round(sw * 10.0) / 10.0),
          @"h" : @(round(sh * 10.0) / 10.0),
        }];
      }
    }
  }

  size_t imgW = CGImageGetWidth(captured);
  size_t imgH = CGImageGetHeight(captured);
  CGImageRelease(captured);

  if (encode_image && !b64)
    return dup_json(@{@"ok" : @NO, @"code" : @"image_encode_failed"});

  return dup_json(@{
    @"ok" : @YES,
    @"bundle_id" : isDesktop ? @"desktop" : (app.bundleIdentifier ?: bid),
    @"frontmost_bundle_id" : frontApp.bundleIdentifier ?: @"",
    @"pid" : @(pid),
    @"window_id" : @(capturedWid),
    @"capture_id" : capId ?: @"",
    @"pixel_digest" : pixelDigest ?: @"",
    @"window_frame" : @{
      @"x" : @(winFrame.origin.x),
      @"y" : @(winFrame.origin.y),
      @"w" : @(winFrame.size.width),
      @"h" : @(winFrame.size.height),
    },
    @"image_width" : @(imgW),
    @"image_height" : @(imgH),
    @"image_format" : (image_format == 1) ? @"jpeg" : @"png",
    @"image_base64" : b64 ?: @"",
    @"png_base64" : b64 ?: @"",
    @"ocr_boxes" : ocrBoxes,
  });
}

char *apexuse_macos_render_som(const char *img_b64_or_cap_id,
                                const char *marks_json, double win_x,
                                double win_y, double win_w, double win_h,
                                int image_format, int quality, int max_width) {
  if ([NSThread isMainThread])
    ensure_cgs_init();
  if (!img_b64_or_cap_id || !marks_json || win_w <= 0 || win_h <= 0) {
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  }
  NSString *inputStr = [NSString stringWithUTF8String:img_b64_or_cap_id];
  NSData *marksData = [[NSString stringWithUTF8String:marks_json]
      dataUsingEncoding:NSUTF8StringEncoding];
  if (!inputStr || !marksData)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});

  NSArray *marks = [NSJSONSerialization JSONObjectWithData:marksData
                                                   options:0
                                                     error:nil];
  if (![marks isKindOfClass:[NSArray class]])
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_marks"});

  CGImageRef baseImg = NULL;
  if ([inputStr hasPrefix:@"cap_"]) {
    baseImg = take_cached_capture(inputStr);
    if (!baseImg)
      return dup_json(@{@"ok" : @NO, @"code" : @"capture_cache_miss"});
  } else {
    NSData *imgData =
        [[NSData alloc] initWithBase64EncodedString:inputStr options:0];
    if (!imgData)
      return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
    CGImageSourceRef src =
        CGImageSourceCreateWithData((__bridge CFDataRef)imgData, NULL);
    if (!src)
      return dup_json(@{@"ok" : @NO, @"code" : @"invalid_image"});
    baseImg = CGImageSourceCreateImageAtIndex(src, 0, NULL);
    CFRelease(src);
    if (!baseImg)
      return dup_json(@{@"ok" : @NO, @"code" : @"invalid_image"});
  }

  size_t srcW = CGImageGetWidth(baseImg);
  size_t srcH = CGImageGetHeight(baseImg);
  size_t imgW = srcW;
  size_t imgH = srcH;
  if (max_width > 0 && srcW > (size_t)max_width) {
    imgW = (size_t)max_width;
    imgH = (size_t)MAX(1.0, round((double)srcH * ((double)max_width / (double)srcW)));
  } else if ([inputStr hasPrefix:@"cap_"] && srcW > 1440) {
    imgW = 1440;
    imgH = (size_t)MAX(1.0, round((double)srcH * (1440.0 / (double)srcW)));
  }
  CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
  CGContextRef ctx = CGBitmapContextCreate(NULL, imgW, imgH, 8, imgW * 4, cs,
                                           (CGBitmapInfo)kCGImageAlphaPremultipliedLast);
  CGColorSpaceRelease(cs);
  if (!ctx) {
    CGImageRelease(baseImg);
    return dup_json(@{@"ok" : @NO, @"code" : @"context_failed"});
  }
  CGContextSetInterpolationQuality(ctx, kCGInterpolationMedium);
  CGContextDrawImage(ctx, CGRectMake(0, 0, imgW, imgH), baseImg);
  CGImageRelease(baseImg);

  CGContextSaveGState(ctx);
  CGContextTranslateCTM(ctx, 0, (CGFloat)imgH);
  CGContextScaleCTM(ctx, 1.0, -1.0);
  NSGraphicsContext *nsCtx =
      [NSGraphicsContext graphicsContextWithCGContext:ctx flipped:YES];
  [NSGraphicsContext saveGraphicsState];
  [NSGraphicsContext setCurrentContext:nsCtx];

  double sx = (double)imgW / win_w;
  double sy = (double)imgH / win_h;
  NSFont *badgeFont = [NSFont boldSystemFontOfSize:11.0];

  for (NSDictionary *m in marks) {
    if (![m isKindOfClass:[NSDictionary class]])
      continue;
    NSString *mid = m[@"id"] ?: @"";
    NSString *source = m[@"source"] ?: @"ax";
    double gx = [m[@"x"] doubleValue];
    double gy = [m[@"y"] doubleValue];
    double gw = [m[@"w"] doubleValue];
    double gh = [m[@"h"] doubleValue];

    double lx = (gx - win_x) * sx;
    double ly = (gy - win_y) * sy;
    double lw = MAX(8.0, gw * sx);
    double lh = MAX(8.0, gh * sy);
    if (lx + lw < 0 || ly + lh < 0 || lx > imgW || ly > imgH)
      continue;

    BOOL isOcr = [source isEqualToString:@"ocr"];
    NSColor *strokeCol = isOcr ? [NSColor colorWithCalibratedRed:0.96
                                                           green:0.62
                                                            blue:0.04
                                                           alpha:0.95]
                               : [NSColor colorWithCalibratedRed:0.06
                                                           green:0.78
                                                            blue:0.65
                                                           alpha:0.95];
    NSColor *fillCol = isOcr ? [NSColor colorWithCalibratedRed:0.96
                                                         green:0.62
                                                          blue:0.04
                                                         alpha:0.12]
                             : [NSColor colorWithCalibratedRed:0.06
                                                         green:0.78
                                                          blue:0.65
                                                         alpha:0.10];

    NSRect box = NSMakeRect(lx, ly, lw, lh);
    [fillCol setFill];
    NSRectFillUsingOperation(box, NSCompositingOperationSourceOver);
    [strokeCol setStroke];
    NSBezierPath *path = [NSBezierPath bezierPathWithRoundedRect:box
                                                         xRadius:3.0
                                                         yRadius:3.0];
    path.lineWidth = 1.8;
    [path stroke];

    NSDictionary *attrs = @{
      NSFontAttributeName : badgeFont,
      NSForegroundColorAttributeName : [NSColor whiteColor],
    };
    NSSize txtSz = [mid sizeWithAttributes:attrs];
    double padX = 5.0;
    double pillW = txtSz.width + padX * 2.0;
    double pillH = 16.0;
    double px = MAX(1.0, MIN((double)imgW - pillW - 1.0, lx));
    double py = (ly >= pillH + 2.0) ? (ly - pillH + 1.0) : MAX(1.0, ly);

    NSRect pillRect = NSMakeRect(px, py, pillW, pillH);
    [[NSColor colorWithCalibratedWhite:0.08 alpha:0.88] setFill];
    [[NSBezierPath bezierPathWithRoundedRect:pillRect xRadius:4.0
                                     yRadius:4.0] fill];
    [strokeCol setStroke];
    NSBezierPath *pillBorder = [NSBezierPath bezierPathWithRoundedRect:pillRect
                                                               xRadius:4.0
                                                               yRadius:4.0];
    pillBorder.lineWidth = 1.0;
    [pillBorder stroke];

    [mid drawAtPoint:NSMakePoint(px + padX, py + 1.0) withAttributes:attrs];
  }

  [NSGraphicsContext restoreGraphicsState];
  CGContextRestoreGState(ctx);

  CGImageRef outImg = CGBitmapContextCreateImage(ctx);
  CGContextRelease(ctx);
  if (!outImg)
    return dup_json(@{@"ok" : @NO, @"code" : @"render_failed"});

  NSString *outB64 = encode_image_b64(outImg, image_format, quality);
  CGImageRelease(outImg);
  if (!outB64)
    return dup_json(@{@"ok" : @NO, @"code" : @"render_failed"});

  return dup_json(@{
    @"ok" : @YES,
    @"image_base64" : outB64,
    @"png_base64" : outB64,
    @"image_format" : (image_format == 1) ? @"jpeg" : @"png"
  });
}

static BOOL point_in_app_window(pid_t pid, CGPoint pt) {
  AXUIElementRef appEl = AXUIElementCreateApplication(pid);
  id winsObj = ax_copy(appEl, kAXWindowsAttribute);
  CFRelease(appEl);
  if (![winsObj isKindOfClass:[NSArray class]])
    return NO;
  for (id w in (NSArray *)winsObj) {
    CGRect f = CGRectZero;
    if (ax_frame((__bridge AXUIElementRef)w, &f) &&
        CGRectContainsPoint(f, pt)) {
      return YES;
    }
  }
  return NO;
}

static BOOL point_in_target_bounds(BOOL isDesktop, pid_t pid, CGPoint pt) {
  if (isDesktop) {
    return CGRectContainsPoint(CGDisplayBounds(CGMainDisplayID()), pt);
  }
  return point_in_app_window(pid, pt);
}

static BOOL is_in_web_content(AXUIElementRef el) {
  AXUIElementRef cur = (AXUIElementRef)CFRetain(el);
  for (int i = 0; i < 24 && cur; i++) {
    NSString *r = ax_string(cur, kAXRoleAttribute);
    if ([r isEqualToString:@"AXWebArea"]) {
      CFRelease(cur);
      return YES;
    }
    CFTypeRef parent = NULL;
    if (AXUIElementCopyAttributeValue(cur, kAXParentAttribute, &parent) !=
            kAXErrorSuccess ||
        !parent) {
      CFRelease(cur);
      break;
    }
    CFRelease(cur);
    cur = (AXUIElementRef)parent;
  }
  return NO;
}

static CGWindowID find_window_at_point(pid_t pid_filter, CGPoint pt,
                                       pid_t *out_pid, CGRect *out_frame) {
  CFArrayRef info = CGWindowListCopyWindowInfo(
      kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
      kCGNullWindowID);
  if (!info)
    return 0;
  NSArray *list = CFBridgingRelease(info);
  CGWindowID matched = 0;
  for (NSDictionary *w in list) {
    int layer = [w[(__bridge NSString *)kCGWindowLayer] intValue];
    if (layer != 0)
      continue;
    pid_t owner = (pid_t)[w[(__bridge NSString *)kCGWindowOwnerPID] intValue];
    if (pid_filter > 0 && owner != pid_filter)
      continue;
    CGRect bounds = CGRectZero;
    if (!CGRectMakeWithDictionaryRepresentation(
            (__bridge CFDictionaryRef)w[(__bridge NSString *)kCGWindowBounds],
            &bounds)) {
      continue;
    }
    if (CGRectContainsPoint(bounds, pt)) {
      matched = (CGWindowID)[w[(
          __bridge NSString *)kCGWindowNumber] unsignedIntValue];
      if (out_pid)
        *out_pid = owner;
      if (out_frame)
        *out_frame = bounds;
      break;
    }
  }
  return matched;
}

static CGWindowID find_primary_window_for_pid(pid_t pid, CGRect *out_frame) {
  CFArrayRef info = CGWindowListCopyWindowInfo(
      kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
      kCGNullWindowID);
  if (!info)
    return 0;
  NSArray *list = CFBridgingRelease(info);
  double maxArea = 0.0;
  for (NSDictionary *w in list) {
    if ([w[(__bridge NSString *)kCGWindowLayer] intValue] != 0)
      continue;
    if ((pid_t)[w[(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid)
      continue;
    NSNumber *alpha = w[(__bridge NSString *)kCGWindowAlpha];
    if (alpha && alpha.doubleValue <= 0.01)
      continue;
    CGRect bounds = CGRectZero;
    if (CGRectMakeWithDictionaryRepresentation(
            (__bridge CFDictionaryRef)w[(__bridge NSString *)kCGWindowBounds],
            &bounds) &&
        bounds.size.width > 120 && bounds.size.height > 140) {
      double a = bounds.size.width * bounds.size.height;
      if (a > maxArea)
        maxArea = a;
    }
  }
  CGWindowID bestWid = 0;
  CGRect bestBounds = CGRectZero;
  double bestScore = -1e18;
  int idx = 0;
  for (NSDictionary *w in list) {
    if ([w[(__bridge NSString *)kCGWindowLayer] intValue] != 0)
      continue;
    if ((pid_t)[w[(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid)
      continue;
    NSNumber *alpha = w[(__bridge NSString *)kCGWindowAlpha];
    if (alpha && alpha.doubleValue <= 0.01)
      continue;
    CGRect bounds = CGRectZero;
    if (!CGRectMakeWithDictionaryRepresentation(
            (__bridge CFDictionaryRef)w[(__bridge NSString *)kCGWindowBounds],
            &bounds) ||
        bounds.size.width <= 10 || bounds.size.height <= 10)
      continue;
    double area = bounds.size.width * bounds.size.height;
    NSString *name = w[(__bridge NSString *)kCGWindowName];
    BOOL named = [name isKindOfClass:[NSString class]] && name.length > 0;
    BOOL major = (maxArea <= 0.0) ? (bounds.size.width > 120 && bounds.size.height > 140)
                                  : (area >= maxArea * 0.45);
    double score = (major ? 1e9 : 0.0) + (named ? 1e6 : 0.0) -
                   (major ? (double)idx * 100.0 : -area);
    if (score > bestScore) {
      bestScore = score;
      bestWid =
          (CGWindowID)[w[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
      bestBounds = bounds;
    }
    idx++;
  }
  if (bestWid != 0 && out_frame)
    *out_frame = bestBounds;
  return bestWid;
}

static NSInteger gAgentMouseEventNum = 100;

static CGEventRef make_pid_mouse_cg_event(pid_t pid, CGWindowID wid,
                                          CGRect winFrame, NSEventType nsType,
                                          CGEventType cgType,
                                          CGMouseButton cgBtn,
                                          CGPoint screenPt, int clickCount,
                                          double pressure) {
  if ([NSThread isMainThread] && wid != 0 && !CGRectIsEmpty(winFrame)) {
    CGFloat mainH = NSScreen.screens.firstObject
                        ? NSScreen.screens.firstObject.frame.size.height
                        : (CGFloat)CGDisplayPixelsHigh(CGMainDisplayID());
    CGFloat localX = MAX(0.0, MIN(winFrame.size.width, screenPt.x - winFrame.origin.x));
    CGFloat localFromTop =
        MAX(0.0, MIN(winFrame.size.height, screenPt.y - winFrame.origin.y));
    NSInteger evNum = ++gAgentMouseEventNum;
    NSEvent *nsEv = [NSEvent
        mouseEventWithType:nsType
                  location:NSMakePoint(localX, mainH - localFromTop)
             modifierFlags:0
                 timestamp:[[NSProcessInfo processInfo] systemUptime]
              windowNumber:(NSInteger)wid
                   context:nil
               eventNumber:evNum
                clickCount:clickCount
                  pressure:(float)pressure];
    if (nsEv && nsEv.CGEvent) {
      CGEventRef cg = CGEventCreateCopy(nsEv.CGEvent);
      if (cg) {
        if (pid > 0) {
          CGEventSetIntegerValueField(cg, kCGEventTargetUnixProcessID,
                                      (int64_t)pid);
        }
        return cg;
      }
    }
  }
  CGEventSourceRef src = CGEventSourceCreate(kCGEventSourceStatePrivate);
  CGEventRef ev = CGEventCreateMouseEvent(src, cgType, screenPt, cgBtn);
  if (ev) {
    CGEventSetFlags(ev, 0);
    CGEventSetIntegerValueField(ev, kCGMouseEventClickState, clickCount);
    CGEventSetDoubleValueField(ev, kCGMouseEventPressure, pressure);
    if (wid != 0) {
      CGEventSetIntegerValueField(ev, (CGEventField)91, (int64_t)wid);
      CGEventSetIntegerValueField(ev, (CGEventField)92, (int64_t)wid);
    }
  }
  if (src)
    CFRelease(src);
  return ev;
}

static void post_mouse_move_to_pid(pid_t pid, CGWindowID wid, CGRect wf,
                                   CGPoint pt) {
  if (pid <= 0)
    return;
  CGEventRef move = make_pid_mouse_cg_event(
      pid, wid, wf, NSEventTypeMouseMoved, kCGEventMouseMoved,
      kCGMouseButtonLeft, pt, 0, 0.0);
  if (move) {
    CGEventPostToPid(pid, move);
    CFRelease(move);
  }
}

static void post_mouse_click_to_pid(pid_t pid, CGWindowID wid, CGRect wf,
                                    CGPoint pt, int clickCount,
                                    BOOL rightButton) {
  if (pid <= 0)
    return;
  NSEventType nsDown =
      rightButton ? NSEventTypeRightMouseDown : NSEventTypeLeftMouseDown;
  NSEventType nsUp =
      rightButton ? NSEventTypeRightMouseUp : NSEventTypeLeftMouseUp;
  CGEventType cgDown =
      rightButton ? kCGEventRightMouseDown : kCGEventLeftMouseDown;
  CGEventType cgUp = rightButton ? kCGEventRightMouseUp : kCGEventLeftMouseUp;
  CGMouseButton btn = rightButton ? kCGMouseButtonRight : kCGMouseButtonLeft;

  post_mouse_move_to_pid(pid, wid, wf, pt);
  usleep(20000);
  for (int i = 1; i <= clickCount; i++) {
    CGEventRef down = make_pid_mouse_cg_event(pid, wid, wf, nsDown, cgDown, btn,
                                              pt, i, 1.0);
    CGEventRef up =
        make_pid_mouse_cg_event(pid, wid, wf, nsUp, cgUp, btn, pt, i, 0.0);
    if (down) {
      CGEventPostToPid(pid, down);
      CFRelease(down);
    }
    usleep(35000);
    if (up) {
      CGEventPostToPid(pid, up);
      CFRelease(up);
    }
    if (i < clickCount)
      usleep(35000);
  }
}

static void post_mouse_drag_to_pid(pid_t pid, CGWindowID wid, CGRect wf,
                                   CGPoint p1, CGPoint p2) {
  if (pid <= 0)
    return;
  post_mouse_move_to_pid(pid, wid, wf, p1);
  usleep(20000);
  CGEventRef down = make_pid_mouse_cg_event(
      pid, wid, wf, NSEventTypeLeftMouseDown, kCGEventLeftMouseDown,
      kCGMouseButtonLeft, p1, 1, 1.0);
  if (down) {
    CGEventPostToPid(pid, down);
    CFRelease(down);
  }
  usleep(25000);
  const int steps = 10;
  for (int i = 1; i <= steps; i++) {
    double t = (double)i / (double)steps;
    CGPoint cur =
        CGPointMake(p1.x + (p2.x - p1.x) * t, p1.y + (p2.y - p1.y) * t);
    CGEventRef drag = make_pid_mouse_cg_event(
        pid, wid, wf, NSEventTypeLeftMouseDragged, kCGEventLeftMouseDragged,
        kCGMouseButtonLeft, cur, 1, 1.0);
    if (drag) {
      CGEventPostToPid(pid, drag);
      CFRelease(drag);
    }
    usleep(12000);
  }
  CGEventRef up = make_pid_mouse_cg_event(
      pid, wid, wf, NSEventTypeLeftMouseUp, kCGEventLeftMouseUp,
      kCGMouseButtonLeft, p2, 1, 0.0);
  if (up) {
    CGEventPostToPid(pid, up);
    CFRelease(up);
  }
}

static inline void flush_unicode_chunk(CGEventSourceRef src, pid_t pid,
                                       unichar *buf, NSUInteger *bufLen,
                                       uint64_t key_delay_us) {
  if (!bufLen || *bufLen == 0)
    return;
  CGEventRef down = CGEventCreateKeyboardEvent(src, 0, true);
  CGEventRef up = CGEventCreateKeyboardEvent(src, 0, false);
  if (down && up) {
    CGEventSetFlags(down, 0);
    CGEventSetFlags(up, 0);
    CGEventKeyboardSetUnicodeString(down, (UniCharCount)*bufLen, buf);
    CGEventKeyboardSetUnicodeString(up, (UniCharCount)*bufLen, buf);
    CGEventPostToPid(pid, down);
    CGEventPostToPid(pid, up);
  }
  if (down)
    CFRelease(down);
  if (up)
    CFRelease(up);
  *bufLen = 0;
  usleep((useconds_t)key_delay_us);
}

static void type_unicode_text_to_pid(pid_t pid, NSString *text,
                                     uint64_t key_delay_us) {
  if (pid <= 0 || text.length == 0)
    return;
  if (key_delay_us == 0)
    key_delay_us = 500;
  CGEventSourceRef src = CGEventSourceCreate(kCGEventSourceStatePrivate);
  unichar buf[16];
  NSUInteger bufLen = 0;

  for (NSUInteger i = 0; i < text.length; i++) {
    unichar ch = [text characterAtIndex:i];
    if (ch == '\n' || ch == '\r' || ch == '\t') {
      flush_unicode_chunk(src, pid, buf, &bufLen, key_delay_us);
      CGKeyCode kc = (ch == '\t') ? 48 : 36;
      CGEventRef down = CGEventCreateKeyboardEvent(src, kc, true);
      CGEventRef up = CGEventCreateKeyboardEvent(src, kc, false);
      if (down && up) {
        CGEventSetFlags(down, 0);
        CGEventSetFlags(up, 0);
        CGEventPostToPid(pid, down);
        CGEventPostToPid(pid, up);
      }
      if (down)
        CFRelease(down);
      if (up)
        CFRelease(up);
      usleep((useconds_t)MAX(1000, key_delay_us));
      continue;
    }
    buf[bufLen++] = ch;
    if (bufLen == 16) {
      flush_unicode_chunk(src, pid, buf, &bufLen, key_delay_us);
    }
  }
  flush_unicode_chunk(src, pid, buf, &bufLen, key_delay_us);
  if (src)
    CFRelease(src);
}

static BOOL press_ax_menu_shortcut_rec(AXUIElementRef el, int depth,
                                       NSString *upperKey, int targetBits) {
  if (depth > 6)
    return NO;
  id childrenObj = ax_copy(el, kAXChildrenAttribute);
  if (![childrenObj isKindOfClass:[NSArray class]])
    return NO;
  for (id child in (NSArray *)childrenObj) {
    AXUIElementRef cel = (__bridge AXUIElementRef)child;
    NSString *cmdChar = ax_string(cel, CFSTR("AXMenuItemCmdChar"));
    NSNumber *cmdMods = ax_bool(cel, CFSTR("AXMenuItemCmdModifiers"));
    int bits = cmdMods ? cmdMods.intValue : 0;
    if (cmdChar && [cmdChar.uppercaseString isEqualToString:upperKey] &&
        bits == targetBits) {
      NSNumber *en = ax_bool(cel, kAXEnabledAttribute);
      if (!en || en.boolValue) {
        return AXUIElementPerformAction(cel, kAXPressAction) == kAXErrorSuccess;
      }
      return NO;
    }
    if (press_ax_menu_shortcut_rec(cel, depth + 1, upperKey, targetBits))
      return YES;
  }
  return NO;
}

static BOOL press_ax_menu_shortcut(pid_t pid, NSString *key, NSArray *mods) {
  BOOL hasCmd = NO;
  int bits = 0;
  for (id m in mods) {
    if (![m isKindOfClass:[NSString class]])
      continue;
    NSString *lm = [(NSString *)m lowercaseString];
    if ([lm isEqualToString:@"cmd"] || [lm isEqualToString:@"command"])
      hasCmd = YES;
    else if ([lm isEqualToString:@"shift"])
      bits |= 1;
    else if ([lm isEqualToString:@"alt"] || [lm isEqualToString:@"option"])
      bits |= 2;
    else if ([lm isEqualToString:@"ctrl"] || [lm isEqualToString:@"control"])
      bits |= 4;
  }
  if (!hasCmd || key.length != 1)
    return NO;
  AXUIElementRef appEl = AXUIElementCreateApplication(pid);
  id barObj = ax_copy(appEl, kAXMenuBarAttribute);
  CFRelease(appEl);
  if (!barObj)
    return NO;
  id itemsObj = ax_copy((__bridge AXUIElementRef)barObj, kAXChildrenAttribute);
  if (![itemsObj isKindOfClass:[NSArray class]])
    return NO;
  NSString *upper = key.uppercaseString;
  NSArray *items = (NSArray *)itemsObj;
  // Skip Apple menu at index 0
  for (NSUInteger i = 1; i < items.count; i++) {
    if (press_ax_menu_shortcut_rec((__bridge AXUIElementRef)items[i], 1, upper,
                                   bits)) {
      return YES;
    }
  }
  return NO;
}

static int keycode_for_name(NSString *key) {
  NSDictionary<NSString *, NSNumber *> *map = @{
    @"return" : @36,
    @"enter" : @36,
    @"tab" : @48,
    @"space" : @49,
    @"delete" : @51,
    @"backspace" : @51,
    @"escape" : @53,
    @"esc" : @53,
    @"forwarddelete" : @117,
    @"home" : @115,
    @"end" : @119,
    @"pageup" : @116,
    @"pagedown" : @121,
    @"left" : @123,
    @"right" : @124,
    @"down" : @125,
    @"up" : @126,
    @"a" : @0,
    @"b" : @11,
    @"c" : @8,
    @"d" : @2,
    @"e" : @14,
    @"f" : @3,
    @"g" : @5,
    @"h" : @4,
    @"i" : @34,
    @"j" : @38,
    @"k" : @40,
    @"l" : @37,
    @"m" : @46,
    @"n" : @45,
    @"o" : @31,
    @"p" : @35,
    @"q" : @12,
    @"r" : @15,
    @"s" : @1,
    @"t" : @17,
    @"u" : @32,
    @"v" : @9,
    @"w" : @13,
    @"x" : @7,
    @"y" : @16,
    @"z" : @6,
    @"0" : @29,
    @"1" : @18,
    @"2" : @19,
    @"3" : @20,
    @"4" : @21,
    @"5" : @23,
    @"6" : @22,
    @"7" : @26,
    @"8" : @28,
    @"9" : @25,
    @"=" : @24,
    @"-" : @27,
    @"[" : @33,
    @"]" : @30,
    @"'" : @39,
    @";" : @41,
    @"\\" : @42,
    @"," : @43,
    @"/" : @44,
    @"." : @47,
    @"`" : @50,
    @"f1" : @122,
    @"f2" : @120,
    @"f3" : @99,
    @"f4" : @118,
    @"f5" : @96,
    @"f6" : @97,
    @"f7" : @98,
    @"f8" : @100,
    @"f9" : @101,
    @"f10" : @109,
    @"f11" : @103,
    @"f12" : @111,
    @"f13" : @105,
    @"f14" : @107,
    @"f15" : @113,
    @"f16" : @106,
    @"f17" : @64,
    @"f18" : @79,
    @"f19" : @80,
    @"f20" : @90,
  };
  NSNumber *code = map[key.lowercaseString];
  return code ? code.intValue : -1;
}

static BOOL is_editable_ax_role(NSString *r) {
  return
      [r isEqualToString:@"AXTextArea"] || [r isEqualToString:@"AXTextField"] ||
      [r isEqualToString:@"AXSearchField"] || [r isEqualToString:@"AXComboBox"];
}

static void spatial_search_point(AXUIElementRef el, BOOL isChrome, BOOL inSheet,
                                 CGPoint pt, int depth, int *scanned,
                                 BOOL *outSafe, AXUIElementRef *bestEl,
                                 NSString **bestRole, int *bestScore,
                                 double *bestArea) {
  if (!*outSafe || *scanned >= 550 || depth > 72)
    return;
  (*scanned)++;

  BOOL sec = NO, unv = NO;
  NSString *role = classify_role(el, isChrome, &sec, &unv);
  if (sec || unv) {
    *outSafe = NO;
    return;
  }
  BOOL nowInSheet = inSheet || [role isEqualToString:@"AXSheet"];

  CGRect f = CGRectZero;
  BOOL hasF = ax_frame(el, &f);
  if (depth >= 1 && hasF) {
    if (f.size.width < 1 || f.size.height < 1 || !CGRectContainsPoint(f, pt)) {
      return;
    }
  }

  if (depth >= 1 && hasF && CGRectContainsPoint(f, pt)) {
    double area = f.size.width * f.size.height;
    NSArray<NSString *> *acts = ax_actions(el);
    int score = 0;
    if (is_editable_ax_role(role))
      score = 3;
    else if (is_interactive_role(role) || [acts containsObject:@"AXOpen"] ||
             ([acts containsObject:@"AXPress"] &&
              ![role isEqualToString:@"AXGroup"]))
      score = 2;
    else if ([acts containsObject:@"AXPress"])
      score = 1;
    if (score > 0 && [acts containsObject:@"AXPress"])
      score += 1;
    if (score > 0 && nowInSheet)
      score += 10;

    if (score > *bestScore ||
        (score == *bestScore && score > 0 && area <= *bestArea)) {
      if (*bestEl)
        CFRelease(*bestEl);
      *bestEl = (AXUIElementRef)CFRetain(el);
      *bestRole = role;
      *bestScore = score;
      *bestArea = area;
    }
  }

  if ([role isEqualToString:@"AXTextArea"])
    return;
  id ch = ax_copy(el, kAXChildrenAttribute);
  if ([ch isKindOfClass:[NSArray class]]) {
    NSArray *arr = (NSArray *)ch;
    NSUInteger lim = MIN(arr.count, (NSUInteger)250);
    for (NSUInteger i = 0; i < lim; i++) {
      spatial_search_point((__bridge AXUIElementRef)arr[i], isChrome,
                           nowInSheet, pt, depth + 1, scanned, outSafe, bestEl,
                           bestRole, bestScore, bestArea);
      if (!*outSafe || *scanned >= 550)
        break;
    }
  }
}

static BOOL check_hit_safe(pid_t pid, BOOL isChrome, double x, double y,
                           AXUIElementRef *outHit, NSString **outRole) {
  if (pid <= 0)
    return YES;
  NSRunningApplication *targetApp =
      [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
  BOOL effectiveChrome =
      isChrome ||
      (targetApp && is_chromium_or_electron(targetApp.bundleIdentifier));
  AXUIElementRef appEl = AXUIElementCreateApplication(pid);
  AXUIElementRef hit = NULL;
  NSString *role = nil;
  BOOL safe = YES;
  if (AXUIElementCopyElementAtPosition(appEl, (float)x, (float)y, &hit) ==
          kAXErrorSuccess &&
      hit) {
    BOOL sec = NO, unv = NO;
    role = classify_role(hit, effectiveChrome, &sec, &unv);
    if (sec || unv) {
      safe = NO;
      CFRelease(hit);
      hit = NULL;
    }
  }

  BOOL needDeepSpatial =
      safe && (!hit || [role isEqualToString:@"AXSplitGroup"] ||
               [role isEqualToString:@"AXScrollArea"] ||
               (effectiveChrome && !is_editable_ax_role(role) &&
                (!is_interactive_role(role) ||
                 ![ax_actions(hit) containsObject:@"AXPress"])));
  if (needDeepSpatial) {
    id winsObj = ax_copy(appEl, kAXWindowsAttribute);
    NSArray *wins =
        [winsObj isKindOfClass:[NSArray class]] ? (NSArray *)winsObj : @[];
    CGPoint pt = CGPointMake(x, y);
    AXUIElementRef bestEl = NULL;
    NSString *bestRole = nil;
    int bestScore = 0;
    double bestArea = 1e18;
    int scanned = 0;
    for (id wObj in wins) {
      AXUIElementRef winEl = (__bridge AXUIElementRef)wObj;
      CGRect wf = CGRectZero;
      if (ax_frame(winEl, &wf) && !CGRectContainsPoint(wf, pt))
        continue;
      spatial_search_point(winEl, effectiveChrome, NO, pt, 0, &scanned, &safe,
                           &bestEl, &bestRole, &bestScore, &bestArea);
      if (!safe)
        break;
    }
    if (!safe) {
      if (bestEl)
        CFRelease(bestEl);
      if (hit)
        CFRelease(hit);
      hit = NULL;
    } else if (bestEl) {
      if (hit)
        CFRelease(hit);
      hit = bestEl;
      role = bestRole;
    }
  }

  if (safe && hit) {
    if (outHit)
      *outHit = hit;
    else
      CFRelease(hit);
    if (outRole)
      *outRole = role;
  } else if (hit) {
    CFRelease(hit);
  }
  CFRelease(appEl);
  return safe;
}

static AXUIElementRef resolve_editable_element(AXUIElementRef hit,
                                               NSString *role, BOOL isChrome,
                                               BOOL *outUnsafe) {
  if (outUnsafe)
    *outUnsafe = NO;
  if (!hit)
    return NULL;
  if (is_editable_ax_role(role)) {
    return (AXUIElementRef)CFRetain(hit);
  }
  AXUIElementRef cur = (AXUIElementRef)CFRetain(hit);
  for (int i = 0; i < 6 && cur; i++) {
    CFTypeRef parent = NULL;
    if (AXUIElementCopyAttributeValue(cur, kAXParentAttribute, &parent) !=
            kAXErrorSuccess ||
        !parent) {
      CFRelease(cur);
      cur = NULL;
      break;
    }
    CFRelease(cur);
    cur = (AXUIElementRef)parent;
    BOOL sec = NO, unv = NO;
    NSString *pr = classify_role(cur, isChrome, &sec, &unv);
    if (sec || unv) {
      if (outUnsafe)
        *outUnsafe = YES;
      CFRelease(cur);
      return NULL;
    }
    if (is_editable_ax_role(pr)) {
      return cur;
    }
  }
  if (cur)
    CFRelease(cur);
  return NULL;
}

static void post_key_chord_to_pid(pid_t pid, int code, CGEventFlags flags) {
  if (pid <= 0 || code < 0)
    return;
  CGEventSourceRef src = CGEventSourceCreate(kCGEventSourceStatePrivate);
  CGEventRef down = CGEventCreateKeyboardEvent(src, (CGKeyCode)code, true);
  CGEventRef up = CGEventCreateKeyboardEvent(src, (CGKeyCode)code, false);
  if (down && up) {
    CGEventSetFlags(down, flags);
    CGEventSetFlags(up, flags);
    CGEventPostToPid(pid, down);
    usleep(18000);
    CGEventPostToPid(pid, up);
  }
  if (down)
    CFRelease(down);
  if (up)
    CFRelease(up);
  if (src)
    CFRelease(src);
}

char *apexuse_macos_perform_action(const char *req_json) {
  ensure_cgs_init();
  if (!AXIsProcessTrusted()) {
    return dup_json(@{@"ok" : @NO, @"code" : @"accessibility_denied"});
  }
  if (!req_json)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSData *data = [[NSString stringWithUTF8String:req_json]
      dataUsingEncoding:NSUTF8StringEncoding];
  NSDictionary *req = data ? [NSJSONSerialization JSONObjectWithData:data
                                                             options:0
                                                               error:nil]
                           : nil;
  if (![req isKindOfClass:[NSDictionary class]])
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});

  NSString *kind = req[@"kind"];
  NSString *bid = req[@"bundle_id"] ?: @"";
  BOOL isDesktop = [bid.lowercaseString isEqualToString:@"desktop"];
  NSRunningApplication *app =
      isDesktop ? [NSWorkspace sharedWorkspace].frontmostApplication
                : resolve_app(bid);
  if (!isDesktop && !app)
    return dup_json(@{@"ok" : @NO, @"code" : @"app_not_running"});
  pid_t pid = app ? app.processIdentifier : 0;
  BOOL isChrome = is_chromium_or_electron(app.bundleIdentifier);

  if ([kind isEqualToString:@"click_point"]) {
    double x = [req[@"x"] doubleValue];
    double y = [req[@"y"] doubleValue];
    int clickCount = MAX(
        1, MIN(3, req[@"click_count"] ? [req[@"click_count"] intValue] : 1));
    BOOL rightBtn = [req[@"button"] isKindOfClass:[NSString class]] &&
                    [[req[@"button"] lowercaseString] isEqualToString:@"right"];
    BOOL preferAx =
        [req[@"prefer_ax"] boolValue] && clickCount == 1 && !rightBtn;
    CGPoint pt = CGPointMake(x, y);
    if (!point_in_target_bounds(isDesktop, pid, pt)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, pt, isDesktop ? &targetPid : NULL, &widFrame);
    AXUIElementRef hit = NULL;
    NSString *role = nil;
    if (!check_hit_safe(targetPid, isChrome, x, y, &hit, &role)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    show_agent_cursor_at(pt, YES);
    pid_t eventPid = targetPid;
    if (hit) {
      pid_t hp = 0;
      if (AXUIElementGetPid(hit, &hp) == kAXErrorSuccess && hp > 0)
        eventPid = hp;
      NSArray<NSString *> *hitActs = ax_actions(hit);
      if (clickCount == 2 && !rightBtn && [hitActs containsObject:@"AXOpen"]) {
        AXError err = AXUIElementPerformAction(hit, CFSTR("AXOpen"));
        CFRelease(hit);
        if (err == kAXErrorSuccess) {
          touch_agent_cursor_ttl();
          return dup_json(
              @{@"ok" : @YES,
                @"method" : @"ax_open",
                @"role" : role ?: @""});
        }
      } else if (preferAx && [hitActs containsObject:@"AXPress"]) {
        AXError err = AXUIElementPerformAction(hit, kAXPressAction);
        CFRelease(hit);
        if (err == kAXErrorSuccess) {
          touch_agent_cursor_ttl();
          return dup_json(
              @{@"ok" : @YES,
                @"method" : @"ax_press",
                @"role" : role ?: @""});
        }
      } else if (preferAx) {
        AXUIElementRef pressable = NULL;
        AXUIElementRef cur = (AXUIElementRef)CFRetain(hit);
        for (int hop = 0; hop < 3 && cur; hop++) {
          CFTypeRef parent = NULL;
          if (AXUIElementCopyAttributeValue(cur, kAXParentAttribute, &parent) !=
                  kAXErrorSuccess ||
              !parent) {
            CFRelease(cur);
            cur = NULL;
            break;
          }
          CFRelease(cur);
          cur = (AXUIElementRef)parent;
          NSArray<NSString *> *pActs = ax_actions(cur);
          if ([pActs containsObject:@"AXPress"]) {
            pressable = cur;
            cur = NULL;
            break;
          }
        }
        if (cur)
          CFRelease(cur);
        CFRelease(hit);
        if (pressable) {
          NSString *pRole = ax_string(pressable, kAXRoleAttribute);
          AXError err = AXUIElementPerformAction(pressable, kAXPressAction);
          CFRelease(pressable);
          if (err == kAXErrorSuccess) {
            touch_agent_cursor_ttl();
            return dup_json(@{
              @"ok" : @YES,
              @"method" : @"ax_press",
              @"role" : pRole ?: (role ?: @"")
            });
          }
        }
      } else {
        CFRelease(hit);
      }
    }
    post_mouse_click_to_pid(eventPid, wid, widFrame, pt, clickCount, rightBtn);
    touch_agent_cursor_ttl();
    return dup_json(@{
      @"ok" : @YES,
      @"method" : rightBtn
          ? @"pid_right_click"
          : (clickCount == 2 ? @"pid_double_click" : @"pid_click")
    });
  }

  if ([kind isEqualToString:@"hover_point"]) {
    double x = [req[@"x"] doubleValue];
    double y = [req[@"y"] doubleValue];
    CGPoint pt = CGPointMake(x, y);
    if (!point_in_target_bounds(isDesktop, pid, pt)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, pt, isDesktop ? &targetPid : NULL, &widFrame);
    show_agent_cursor_at(pt, NO);
    post_mouse_move_to_pid(targetPid, wid, widFrame, pt);
    touch_agent_cursor_ttl();
    return dup_json(@{@"ok" : @YES, @"method" : @"agent_cursor_hover"});
  }

  if ([kind isEqualToString:@"drag_points"]) {
    double x1 = [req[@"x1"] doubleValue], y1 = [req[@"y1"] doubleValue];
    double x2 = [req[@"x2"] doubleValue], y2 = [req[@"y2"] doubleValue];
    CGPoint p1 = CGPointMake(x1, y1), p2 = CGPointMake(x2, y2);
    if (!point_in_target_bounds(isDesktop, pid, p1) ||
        !point_in_target_bounds(isDesktop, pid, p2)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, p1, isDesktop ? &targetPid : NULL, &widFrame);
    if (!check_hit_safe(targetPid, isChrome, x1, y1, NULL, NULL) ||
        !check_hit_safe(targetPid, isChrome, x2, y2, NULL, NULL)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    show_agent_cursor_at(p1, YES);
    post_mouse_drag_to_pid(targetPid, wid, widFrame, p1, p2);
    show_agent_cursor_at(p2, NO);
    touch_agent_cursor_ttl();
    return dup_json(@{@"ok" : @YES, @"method" : @"pid_drag"});
  }

  if ([kind isEqualToString:@"type_point"]) {
    double x = [req[@"x"] doubleValue];
    double y = [req[@"y"] doubleValue];
    NSString *text = req[@"text"] ?: @"";
    NSString *mode = req[@"mode"] ?: @"insert";
    BOOL clickFirst = [req[@"click_first"] boolValue];
    uint64_t keyDelayUs = req[@"key_delay_us"]
                              ? [req[@"key_delay_us"] unsignedLongLongValue]
                              : 500;
    CGPoint pt = CGPointMake(x, y);
    if (!point_in_target_bounds(isDesktop, pid, pt)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, pt, isDesktop ? &targetPid : NULL, &widFrame);
    AXUIElementRef hit = NULL;
    NSString *role = nil;
    if (!check_hit_safe(targetPid, isChrome, x, y, &hit, &role)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    show_agent_cursor_at(pt, clickFirst);
    pid_t eventPid = targetPid;
    if (hit) {
      pid_t hp = 0;
      if (AXUIElementGetPid(hit, &hp) == kAXErrorSuccess && hp > 0)
        eventPid = hp;
    }
    if (hit && !clickFirst) {
      BOOL unsafeParent = NO;
      AXUIElementRef editable =
          resolve_editable_element(hit, role, isChrome, &unsafeParent);
      BOOL webTarget = isChrome || is_in_web_content(hit);
      CFRelease(hit);
      if (unsafeParent) {
        touch_agent_cursor_ttl();
        return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
      }
      if (!editable) {
        touch_agent_cursor_ttl();
        return dup_json(
            @{@"ok" : @NO,
              @"code" : @"target_identity_unverified"});
      }
      if ([mode isEqualToString:@"replace"]) {
        AXUIElementSetAttributeValue(editable, kAXFocusedAttribute,
                                     kCFBooleanTrue);
        AXError sErr = AXUIElementSetAttributeValue(editable, kAXValueAttribute,
                                                    (__bridge CFTypeRef)text);
        CFRelease(editable);
        if (sErr == kAXErrorSuccess) {
          touch_agent_cursor_ttl();
          return dup_json(@{@"ok" : @YES, @"method" : @"ax_set_value"});
        }
        if (!webTarget) {
          touch_agent_cursor_ttl();
          return dup_json(@{@"ok" : @NO, @"code" : @"replacement_set_failed"});
        }
        post_mouse_click_to_pid(eventPid, wid, widFrame, pt, 1, NO);
        usleep(50000);
        post_key_chord_to_pid(eventPid, 0 /* a */, kCGEventFlagMaskCommand);
        usleep(25000);
        post_key_chord_to_pid(eventPid, 51 /* delete */, 0);
        usleep(25000);
        type_unicode_text_to_pid(eventPid, text, keyDelayUs);
        touch_agent_cursor_ttl();
        return dup_json(@{@"ok" : @YES, @"method" : @"pid_replace_text"});
      }
      AXUIElementSetAttributeValue(editable, kAXFocusedAttribute,
                                   kCFBooleanTrue);
      if (webTarget && [ax_actions(editable) containsObject:@"AXPress"]) {
        AXUIElementPerformAction(editable, kAXPressAction);
      }
      CFRelease(editable);
      if (webTarget) {
        post_mouse_click_to_pid(eventPid, wid, widFrame, pt, 1, NO);
        usleep(50000);
      }
    } else {
      if (hit) {
        AXUIElementSetAttributeValue(hit, kAXFocusedAttribute, kCFBooleanTrue);
        if ((isChrome || is_in_web_content(hit)) &&
            [ax_actions(hit) containsObject:@"AXPress"]) {
          AXUIElementPerformAction(hit, kAXPressAction);
        }
        CFRelease(hit);
      }
      post_mouse_click_to_pid(eventPid, wid, widFrame, pt, 1, NO);
      usleep(60000);
    }
    type_unicode_text_to_pid(eventPid, text, keyDelayUs);
    touch_agent_cursor_ttl();
    return dup_json(@{@"ok" : @YES, @"method" : @"pid_type_text"});
  }

  if ([kind isEqualToString:@"set_value"]) {
    double x = [req[@"x"] doubleValue];
    double y = [req[@"y"] doubleValue];
    NSString *text = req[@"text"] ?: @"";
    uint64_t keyDelayUs = req[@"key_delay_us"]
                              ? [req[@"key_delay_us"] unsignedLongLongValue]
                              : 500;
    CGPoint pt = CGPointMake(x, y);
    if (!point_in_target_bounds(isDesktop, pid, pt)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, pt, isDesktop ? &targetPid : NULL, &widFrame);
    AXUIElementRef hit = NULL;
    NSString *role = nil;
    if (!check_hit_safe(targetPid, isChrome, x, y, &hit, &role)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    pid_t eventPid = targetPid;
    if (hit) {
      pid_t hp = 0;
      if (AXUIElementGetPid(hit, &hp) == kAXErrorSuccess && hp > 0)
        eventPid = hp;
    }
    BOOL unsafeParent = NO;
    AXUIElementRef editable =
        resolve_editable_element(hit, role, isChrome, &unsafeParent);
    BOOL webTarget = isChrome || (hit && is_in_web_content(hit));
    if (hit)
      CFRelease(hit);
    if (unsafeParent) {
      touch_agent_cursor_ttl();
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    if (!editable) {
      touch_agent_cursor_ttl();
      return dup_json(
          @{@"ok" : @NO, @"code" : @"target_identity_unverified"});
    }
    AXUIElementSetAttributeValue(editable, kAXFocusedAttribute, kCFBooleanTrue);
    AXError sErr = AXUIElementSetAttributeValue(editable, kAXValueAttribute,
                                                (__bridge CFTypeRef)text);
    CFRelease(editable);
    if (sErr == kAXErrorSuccess) {
      touch_agent_cursor_ttl();
      return dup_json(@{@"ok" : @YES, @"method" : @"ax_set_value"});
    }
    if (!webTarget) {
      touch_agent_cursor_ttl();
      return dup_json(@{
        @"ok" : @NO,
        @"code" : @"replacement_set_failed",
        @"message" : @"Direct AX value write failed on native control."
      });
    }
    show_agent_cursor_at(pt, YES);
    post_mouse_click_to_pid(eventPid, wid, widFrame, pt, 1, NO);
    usleep(50000);
    post_key_chord_to_pid(eventPid, 0 /* a */, kCGEventFlagMaskCommand);
    usleep(25000);
    post_key_chord_to_pid(eventPid, 51 /* delete */, 0);
    usleep(25000);
    if (text.length > 0) {
      type_unicode_text_to_pid(eventPid, text, keyDelayUs);
    }
    touch_agent_cursor_ttl();
    return dup_json(@{@"ok" : @YES, @"method" : @"pid_set_value_fallback"});
  }

  if ([kind isEqualToString:@"select_text"]) {
    double x = [req[@"x"] doubleValue];
    double y = [req[@"y"] doubleValue];
    NSNumber *startNum = req[@"start"];
    NSNumber *endNum = req[@"end"];
    CGPoint pt = CGPointMake(x, y);
    if (!point_in_target_bounds(isDesktop, pid, pt)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
    }
    pid_t targetPid = pid;
    CGRect widFrame = CGRectZero;
    CGWindowID wid = find_window_at_point(
        isDesktop ? 0 : pid, pt, isDesktop ? &targetPid : NULL, &widFrame);
    AXUIElementRef hit = NULL;
    NSString *role = nil;
    if (!check_hit_safe(targetPid, isChrome, x, y, &hit, &role)) {
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    pid_t eventPid = targetPid;
    if (hit) {
      pid_t hp = 0;
      if (AXUIElementGetPid(hit, &hp) == kAXErrorSuccess && hp > 0)
        eventPid = hp;
    }
    BOOL unsafeParent = NO;
    AXUIElementRef editable =
        resolve_editable_element(hit, role, isChrome, &unsafeParent);
    BOOL webTarget = isChrome || (hit && is_in_web_content(hit));
    if (hit)
      CFRelease(hit);
    if (unsafeParent) {
      touch_agent_cursor_ttl();
      return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
    }
    if (!editable) {
      touch_agent_cursor_ttl();
      return dup_json(
          @{@"ok" : @NO, @"code" : @"target_identity_unverified"});
    }
    AXUIElementSetAttributeValue(editable, kAXFocusedAttribute, kCFBooleanTrue);
    NSString *valStr = ax_string(editable, kAXValueAttribute) ?: @"";
    NSInteger strLen = (NSInteger)valStr.length;
    NSInteger s = (startNum && ![startNum isKindOfClass:[NSNull class]])
                      ? [startNum integerValue]
                      : 0;
    NSInteger e = (endNum && ![endNum isKindOfClass:[NSNull class]])
                      ? [endNum integerValue]
                      : strLen;
    s = MAX(0, MIN(strLen, s));
    e = MAX(s, MIN(strLen, e));
    CFRange range = CFRangeMake(s, e - s);
    AXValueRef valRef = AXValueCreate(kAXValueCFRangeType, &range);
    AXError rErr = kAXErrorFailure;
    if (valRef) {
      rErr = AXUIElementSetAttributeValue(editable,
                                          kAXSelectedTextRangeAttribute, valRef);
      CFRelease(valRef);
    }
    CFRelease(editable);
    if (rErr == kAXErrorSuccess) {
      touch_agent_cursor_ttl();
      return dup_json(@{@"ok" : @YES, @"method" : @"ax_select_text"});
    }
    BOOL isSelectAll =
        (!startNum || [startNum isKindOfClass:[NSNull class]] || s == 0) &&
        (!endNum || [endNum isKindOfClass:[NSNull class]] || e >= strLen);
    if (webTarget && isSelectAll) {
      show_agent_cursor_at(pt, YES);
      post_mouse_click_to_pid(eventPid, wid, widFrame, pt, 1, NO);
      usleep(50000);
      post_key_chord_to_pid(eventPid, 0 /* a */, kCGEventFlagMaskCommand);
      touch_agent_cursor_ttl();
      return dup_json(@{@"ok" : @YES, @"method" : @"pid_select_all"});
    }
    touch_agent_cursor_ttl();
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"select_text_failed",
      @"message" : @"Failed to set text selection range."
    });
  }

  if ([kind isEqualToString:@"press_key"]) {
    NSString *key = req[@"key"] ?: @"";
    NSArray *mods = [req[@"modifiers"] isKindOfClass:[NSArray class]]
                        ? req[@"modifiers"]
                        : @[];
    if (pid > 0 && press_ax_menu_shortcut(pid, key, mods)) {
      return dup_json(@{@"ok" : @YES, @"method" : @"ax_menu"});
    }
    int code = keycode_for_name(key);
    if (code < 0)
      return dup_json(@{@"ok" : @NO, @"code" : @"unsupported_key"});
    CGEventFlags flags = 0;
    for (id m in mods) {
      if (![m isKindOfClass:[NSString class]])
        continue;
      NSString *lm = [(NSString *)m lowercaseString];
      if ([lm isEqualToString:@"cmd"] || [lm isEqualToString:@"command"])
        flags |= kCGEventFlagMaskCommand;
      else if ([lm isEqualToString:@"ctrl"] || [lm isEqualToString:@"control"])
        flags |= kCGEventFlagMaskControl;
      else if ([lm isEqualToString:@"alt"] || [lm isEqualToString:@"option"])
        flags |= kCGEventFlagMaskAlternate;
      else if ([lm isEqualToString:@"shift"])
        flags |= kCGEventFlagMaskShift;
    }
    post_key_chord_to_pid(pid, code, flags);
    return dup_json(@{@"ok" : @YES, @"method" : @"pid_key"});
  }

  if ([kind isEqualToString:@"scroll"]) {
    NSString *dir = req[@"direction"] ?: @"down";
    int amount = MAX(1, MIN(100, [req[@"amount"] intValue]));
    int32_t dy = 0, dx = 0;
    if ([dir isEqualToString:@"down"])
      dy = -amount;
    else if ([dir isEqualToString:@"up"])
      dy = amount;
    else if ([dir isEqualToString:@"right"])
      dx = -amount;
    else if ([dir isEqualToString:@"left"])
      dx = amount;
    CGRect wf = CGRectZero;
    CGWindowID wid = (pid > 0) ? find_primary_window_for_pid(pid, &wf) : 0;
    CGPoint pt = !CGRectIsEmpty(wf)
                     ? CGPointMake(CGRectGetMidX(wf), CGRectGetMidY(wf))
                     : CGPointZero;
    if (req[@"x"] && req[@"y"]) {
      pt = CGPointMake([req[@"x"] doubleValue], [req[@"y"] doubleValue]);
      if (!point_in_target_bounds(isDesktop, pid, pt)) {
        return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
      }
      pid_t pTmp = pid;
      CGWindowID hitWid = find_window_at_point(
          isDesktop ? 0 : pid, pt, isDesktop ? &pTmp : NULL, &wf);
      if (hitWid != 0) {
        wid = hitWid;
        pid = pTmp;
      }
    }
    if (!CGRectIsEmpty(wf)) {
      show_agent_cursor_at(pt, NO);
      post_mouse_move_to_pid(pid, wid, wf, pt);
      usleep(15000);
    }
    CGEventRef ev = NULL;
    if ([NSThread isMainThread] && wid != 0 && !CGRectIsEmpty(wf)) {
      CGEventRef base = make_pid_mouse_cg_event(
          pid, wid, wf, NSEventTypeMouseMoved, kCGEventMouseMoved,
          kCGMouseButtonLeft, pt, 0, 0.0);
      if (base) {
        CGEventSetType(base, kCGEventScrollWheel);
        CGEventSetIntegerValueField(base, kCGScrollWheelEventDeltaAxis1, dy);
        CGEventSetIntegerValueField(base, kCGScrollWheelEventDeltaAxis2, dx);
        CGEventSetIntegerValueField(base, kCGScrollWheelEventPointDeltaAxis1,
                                    (int64_t)dy * 12);
        CGEventSetIntegerValueField(base, kCGScrollWheelEventPointDeltaAxis2,
                                    (int64_t)dx * 12);
        CGEventSetIntegerValueField(base, kCGScrollWheelEventIsContinuous, 1);
        ev = base;
      }
    }
    if (!ev) {
      CGEventSourceRef src = CGEventSourceCreate(kCGEventSourceStatePrivate);
      ev = CGEventCreateScrollWheelEvent(src, kCGScrollEventUnitLine, 2, dy, dx);
      if (ev && !CGRectIsEmpty(wf))
        CGEventSetLocation(ev, pt);
      if (ev && wid != 0) {
        CGEventSetIntegerValueField(ev, (CGEventField)91, (int64_t)wid);
        CGEventSetIntegerValueField(ev, (CGEventField)92, (int64_t)wid);
      }
      if (src)
        CFRelease(src);
    }
    if (ev && pid > 0) {
      CGEventPostToPid(pid, ev);
      touch_agent_cursor_ttl();
    }
    if (ev)
      CFRelease(ev);
    return dup_json(@{@"ok" : @YES, @"method" : @"pid_scroll"});
  }

  return dup_json(@{@"ok" : @NO, @"code" : @"unsupported_action"});
}

char *apexuse_macos_check_point(const char *bundle_id, double x, double y) {
  if (!AXIsProcessTrusted()) {
    return dup_json(@{@"ok" : @NO, @"code" : @"accessibility_denied"});
  }
  if (!bundle_id)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *bid = [NSString stringWithUTF8String:bundle_id];
  BOOL isDesktop = [bid.lowercaseString isEqualToString:@"desktop"];
  NSRunningApplication *app =
      isDesktop ? [NSWorkspace sharedWorkspace].frontmostApplication
                : resolve_app(bid);
  if (!isDesktop && !app)
    return dup_json(@{@"ok" : @NO, @"code" : @"app_not_running"});
  pid_t pid = app ? app.processIdentifier : 0;
  BOOL isChrome = is_chromium_or_electron(app.bundleIdentifier);
  CGPoint pt = CGPointMake(x, y);
  if (!point_in_target_bounds(isDesktop, pid, pt)) {
    return dup_json(@{@"ok" : @NO, @"code" : @"coordinate_outside_window"});
  }
  pid_t targetPid = pid;
  if (isDesktop) {
    find_window_at_point(0, pt, &targetPid, NULL);
  }
  NSString *role = nil;
  if (!check_hit_safe(targetPid, isChrome, x, y, NULL, &role)) {
    return dup_json(@{@"ok" : @NO, @"code" : @"unsafe_target_field"});
  }
  return dup_json(@{
    @"ok" : @YES,
    @"frontmost_bundle_id" :
        [NSWorkspace sharedWorkspace].frontmostApplication.bundleIdentifier
        ?: @"",
    @"role" : role ?: @"",
  });
}

char *apexuse_macos_open_url(const char *bundle_id, const char *url) {
  if (!url)
    return dup_json(@{@"ok" : @NO, @"code" : @"invalid_argument"});
  NSString *u = [NSString stringWithUTF8String:url];
  NSString *bid = bundle_id ? [NSString stringWithUTF8String:bundle_id] : @"";
  NSTask *task = [[NSTask alloc] init];
  task.executableURL = [NSURL fileURLWithPath:@"/usr/bin/open"];
  if (bid.length > 0 && ![bid.lowercaseString isEqualToString:@"desktop"]) {
    task.arguments = @[ @"-b", bid, u ];
  } else {
    task.arguments = @[ u ];
  }
  NSError *err = nil;
  if (![task launchAndReturnError:&err]) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"open_url_failed",
      @"message" : err.localizedDescription ?: @"open failed"
    });
  }
  [task waitUntilExit];
  if (task.terminationStatus != 0) {
    return dup_json(@{
      @"ok" : @NO,
      @"code" : @"open_url_failed",
      @"message" : @"open exited nonzero"
    });
  }
  return dup_json(@{@"ok" : @YES, @"url" : u, @"bundle_id" : bid});
}
