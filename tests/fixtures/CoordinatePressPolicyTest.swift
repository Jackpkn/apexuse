import Foundation
func check(_ value: Bool, _ message: String) { if !value { fatalError(message) } }
check(permitsCoordinatePress(role: "AXButton", enabled: true, actions: ["AXPress"], inWeb: false), "button must be pressable")
check(permitsCoordinatePress(role: "AXCheckBox", enabled: true, actions: ["AXPress"], inWeb: false), "checkbox must be pressable")
check(!permitsCoordinatePress(role: "AXButton", enabled: false, actions: ["AXPress"], inWeb: false), "disabled button must be denied")
check(!permitsCoordinatePress(role: "AXButton", enabled: nil, actions: ["AXPress"], inWeb: false), "unknown enabled state must be denied")
check(!permitsCoordinatePress(role: "AXSecureTextField", enabled: true, actions: ["AXPress"], inWeb: false), "secure field must be denied")
check(!permitsCoordinatePress(role: "AXButton", enabled: true, actions: [], inWeb: false), "missing press action must be denied")
check(!permitsCoordinatePress(role: "AXButton", enabled: true, actions: ["AXPress"], inWeb: true), "web targets must use the existing fallback")
check(pointInTargetWindow(point: CGPoint(x: 50, y: 50), frame: CGRect(x: 0, y: 0, width: 100, height: 100)), "point inside target must be allowed")
check(!pointInTargetWindow(point: CGPoint(x: 150, y: 50), frame: CGRect(x: 0, y: 0, width: 100, height: 100)), "point outside target must be denied")
print("Coordinate target policy passed")
