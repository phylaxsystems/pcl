// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Assertion} from "../../lib/credible-std/src/Assertion.sol";
import {SimpleLending} from "../../src/SimpleLending.sol";
import {IPriceFeed} from "../../src/SimpleLending.sol";
import {console} from "../../lib/credible-std/lib/forge-std/src/console.sol";
import {PhEvm} from "../../lib/credible-std/src/PhEvm.sol";

contract SimpleLendingAssertion is Assertion {
    SimpleLending simpleLending;
    address[] public whitelistedAddresses;
    address public bybitSafeAddress;

    constructor(address simpleLending_) {
        simpleLending = SimpleLending(simpleLending_);
    }

    function triggers() external view override {
        registerCallTrigger(this.assertionSafeDrain.selector);
    }

    function assertionSafeDrain() external {
        ph.forkPreState();
        uint256 preBalance = address(bybitSafeAddress).balance;
        uint256[] memory preWhitelistBalances = new uint256[](whitelistedAddresses.length);
        for (uint256 i = 0; i < whitelistedAddresses.length; i++) {
            preWhitelistBalances[i] = address(whitelistedAddresses[i]).balance;
        }

        ph.forkPostState();
        uint256 postBalance = address(bybitSafeAddress).balance;
        if (postBalance > preBalance) {
            return; // Balance increased, not a hack
        }
        uint256 diff = preBalance - postBalance;
        for (uint256 i = 0; i < whitelistedAddresses.length; i++) {
            uint256 postWhitelistBalance = address(whitelistedAddresses[i]).balance;
            if (postWhitelistBalance == preWhitelistBalances[i] + diff) {
                return; // Balance increased, not a hack
            }
        }
        // None of the whitelisted addresses have increased in balance, so it's a hack
        revert("Bybit safe address balance decreased");
    }
}
