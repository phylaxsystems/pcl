// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Assertion} from "credible-std/Assertion.sol";
import {MockProtocol} from "../../src/protocol.sol";

contract MockAssertion is Assertion {
    MockProtocol immutable protocol;

    constructor(address protocol_) {
        protocol = MockProtocol(protocol_);
    }

    function fnSelectors()
        external
        pure
        override
        returns (bytes4[] memory assertions)
    {
        assertions = new bytes4[](1);
        assertions[0] = this.assertionCheckBool.selector;
    }

    function assertionCheckBool() external view returns (bool) {
        return protocol.checkBool();
    }
}
