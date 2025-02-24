// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "../../lib/credible-std/lib/forge-std/src/Test.sol";
import {CredibleTest} from "../../lib/credible-std/src/CredibleTest.sol";
import {MockAssertion} from "../src/MockAssertion.sol";
import {MockProtocol} from "../../src/protocol.sol";

contract TestMockAssertion is Test, CredibleTest {
    MockProtocol public protocol;
    address testAddress = address(0xBEEF00000);

    function setUp() public {
        protocol = new MockProtocol();
        vm.deal(testAddress, 1000 ether);
    }

    function test_assertionCheckBool() public {
        // Register the assertion
        cl.addAssertion("assertionCheckBool", address(protocol), type(MockAssertion).creationCode, abi.encode(protocol));

        // Validate should pass since checkBool() returns true by default
        vm.prank(testAddress);
        cl.validate("assertionCheckBool", address(protocol), 0, abi.encodeWithSelector(protocol.checkBool.selector));
    }
}
