// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Assertion} from "credible-std/Assertion.sol";

contract MockAssertion is Assertion {

    function fnSelectors() public pure override returns (Trigger[] memory) {
        return new Trigger[](1);
    }

    function assertion_example_mock() public {
    }
}
