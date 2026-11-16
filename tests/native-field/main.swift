import ApplicationServices
func check(_ role: String?, _ subrole: String?, _ status: AXError, _ expected: String) {
    let actual = handsmcpFieldRole(role: role, subrole: subrole, subroleStatus: status)
    precondition(actual == expected, "got \(actual), expected \(expected)")
}
check("AXTextField", nil, .noValue, "AXTextField")
check("AXTextField", nil, .cannotComplete, "AXUnverifiedTextField")
check("AXTextField", nil, .attributeUnsupported, "AXUnverifiedTextField")
check("AXTextField", "AXSecureTextField", .success, "AXSecureTextField")
check("AXSecureTextField", nil, .noValue, "AXSecureTextField")
check(nil, nil, .cannotComplete, "AXUnverifiedTextField")
print("field classification passed")
precondition(handsmcpFieldRole(role: "AXTextField", subrole: nil, subroleStatus: .noValue, chrome: false) == "AXUnverifiedTextField")
precondition(handsmcpFieldRole(role: "AXTextField", subrole: nil, subroleStatus: .success, chrome: true) == "AXUnverifiedTextField")
