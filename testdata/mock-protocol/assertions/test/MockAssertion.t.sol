// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "../../lib/credible-std/lib/forge-std/src/Test.sol";
import {Credible} from "../../lib/credible-std/src/Credible.sol";
import {MockAssertion} from "../src/MockAssertion.sol";

contract TestMockAssertion is Test, Credible {
    function test_assertionCheckBool() public {
        MockAssertion assertion = new MockAssertion();
        assertEq(assertion.assertionCheckBool(), true);
    }
}
