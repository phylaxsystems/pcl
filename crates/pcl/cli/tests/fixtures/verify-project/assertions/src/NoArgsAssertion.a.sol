// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface TriggerRecorder {
    function registerCallTrigger(bytes4 fnSelector) external view;
}

abstract contract Assertion {
    TriggerRecorder constant triggerRecorder =
        TriggerRecorder(address(uint160(uint256(keccak256("TriggerRecorder")))));

    function triggers() external view virtual;

    function registerCallTrigger(bytes4 fnSelector) internal view {
        triggerRecorder.registerCallTrigger(fnSelector);
    }
}

contract NoArgsAssertion is Assertion {
    function triggers() external view override {
        registerCallTrigger(this.assertionCheckBool.selector);
    }

    function assertionCheckBool() external pure returns (bool) {
        return true;
    }
}

