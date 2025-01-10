// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Assertion} from "credible-std/Assertion.sol";
import {MockProtocol} from "../../src/protocol.sol";

contract MockAssertion is Assertion, MockProtocol {
    function fnSelectors()
        external
        pure
        override
        returns (bytes4[] memory assertions)
    {
        assertions = new bytes4[](1);
        assertions[0] = this.assertionCheckBool.selector;
    }

    function assertionCheckBool() external returns (bool) {
        return checkBool();
    }
}
