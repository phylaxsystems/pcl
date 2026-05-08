// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import "../src/NoArgsAssertion.a.sol";

contract NoArgsAssertionTest {
    function testAssertionCheckBool() public {
        NoArgsAssertion assertion = new NoArgsAssertion();
        require(assertion.assertionCheckBool(), "expected assertion to pass");
    }
}
