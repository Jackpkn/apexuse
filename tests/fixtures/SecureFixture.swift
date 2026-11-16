import AppKit
import Foundation
final class Fixture: NSObject, NSApplicationDelegate {
    var window: NSWindow?
    func applicationDidFinishLaunching(_ notification: Notification) {
        let window = NSWindow(contentRect: NSRect(x: 300, y: 300, width: 420, height: 140), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "handsmcp secure field fixture"
        let field = NSSecureTextField(frame: NSRect(x: 20, y: 50, width: 360, height: 26))
        field.stringValue = "SYNTHETIC_TEST_VALUE"
        window.contentView?.addSubview(field)
        self.window = window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}
let app = NSApplication.shared
let delegate = Fixture()
app.delegate = delegate
app.setActivationPolicy(.regular)
withExtendedLifetime(delegate) { app.run() }
