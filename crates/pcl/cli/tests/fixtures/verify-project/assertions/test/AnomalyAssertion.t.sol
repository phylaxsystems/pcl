// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface AnomalyTriggerRecorder {
    function watchAnomaly(address target, bytes4 fnSelector, uint8 sensitivity) external view;
}

interface AnomalyVm {
    function assertion(address adopter, bytes calldata createData, bytes4 fnSelector) external;
    function setAnomalyLevel(address target, uint8 firesAt) external;
    function expectRevert(bytes calldata revertData) external;
}

contract AnomalyTarget {
    function touch() external {}
}

// Watches at level 7 and always reverts, so a test sees only whether the trigger fired.
contract AnomalyAssertion {
    AnomalyTriggerRecorder private constant TRIGGER_RECORDER =
        AnomalyTriggerRecorder(address(uint160(uint256(keccak256("TriggerRecorder")))));

    address private immutable target;

    constructor(address target_) {
        target = target_;
    }

    function triggers() external view {
        TRIGGER_RECORDER.watchAnomaly(target, this.assertNotAnomalous.selector, 7);
    }

    function assertNotAnomalous() external pure {
        revert("anomaly trigger fired");
    }
}

contract AnomalyAssertionTest {
    AnomalyVm private constant CL =
        AnomalyVm(address(uint160(uint256(keccak256("hevm cheat code")))));

    function testVerdictAtWatchedLevelFires() public {
        _expect(7, "anomaly trigger fired");
    }

    function testLooserVerdictDoesNotFire() public {
        _expect(8, "Expected 1 assertion to be executed, but 0 were executed.");
    }

    function _expect(uint8 firesAt, bytes memory reason) private {
        AnomalyTarget target = new AnomalyTarget();
        CL.setAnomalyLevel(address(target), firesAt);
        CL.assertion(
            address(target),
            abi.encodePacked(type(AnomalyAssertion).creationCode, abi.encode(address(target))),
            AnomalyAssertion.assertNotAnomalous.selector
        );
        CL.expectRevert(reason);
        target.touch();
    }
}
