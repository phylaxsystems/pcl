// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Assertion} from "../../lib/credible-std/src/Assertion.sol";
import {IPriceFeed} from "../../src/SimpleLending.sol";

contract PriceFeedAssertion is Assertion {
    IPriceFeed ethPriceFeed;
    IPriceFeed tokenPriceFeed;

    constructor(address ethPriceFeed_, address tokenPriceFeed_) {
        ethPriceFeed = IPriceFeed(ethPriceFeed_);
        tokenPriceFeed = IPriceFeed(tokenPriceFeed_);
    }

    function triggers() external view override {
        registerCallTrigger(this.assertionPriceDeviation.selector);
    }

    function assertionPriceDeviation() external {
        ph.forkPreState();
        uint256 preTokenPrice = tokenPriceFeed.getPrice();

        ph.forkPostState();
        uint256 postTokenPrice = tokenPriceFeed.getPrice();
    }
}
