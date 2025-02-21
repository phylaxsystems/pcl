// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Assertion} from "credible-std/Assertion.sol";
import {MockProtocol} from "../../src/protocol.sol";

contract MockAssertion is Assertion {
    MockProtocol immutable protocol;

    constructor(MockProtocol protocol_) {
        protocol = protocol_;
    }

    function triggers() external view override {
        registerCallTrigger(this.assertionCheckBool.selector);
    }

    function assertionCheckBool() external {
        ph.forkPostState();
        require(protocol.checkBool(), "Assertion failed");
    }
}
