// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Assertion} from "../../lib/credible-std/src/Assertion.sol";
import {SimpleLending} from "../../src/SimpleLending.sol";
import {IPriceFeed} from "../../src/SimpleLending.sol";

contract SimpleLendingAssertion is Assertion {
    SimpleLending simpleLending;
    IPriceFeed ethPriceFeed;
    IPriceFeed tokenPriceFeed;

    constructor(address simpleLending_) {
        simpleLending = SimpleLending(simpleLending_);
    }

    function triggers() external view override {
        registerCallTrigger(this.assertionCollateralBalance.selector);
        registerCallTrigger(this.assertionBorrowedInvariant.selector);
        registerCallTrigger(this.assertionPriceDeviation.selector);
        registerCallTrigger(this.assertionEthDrain.selector);
    }

    function assertionCollateralBalance() external {
        ph.forkPreState();
        (uint256 collateralAmount, uint256 borrowedAmount) = simpleLending.positions(msg.sender);
        uint256 preCollateralBalance = collateralAmount;
        uint256 ethPrice = ethPriceFeed.getPrice();
        uint256 tokenPrice = tokenPriceFeed.getPrice();

        ph.forkPostState();
        uint256 postCollateralBalance = collateralAmount;
        uint256 collateralChange;
        if (postCollateralBalance > preCollateralBalance) {
            collateralChange = postCollateralBalance - preCollateralBalance; // Added collateral
        } else {
            collateralChange = preCollateralBalance - postCollateralBalance; // Removed collateral
        }

        uint256 newCollateralValue = (collateralAmount - collateralChange) * ethPrice;
        uint256 borrowedValue = borrowedAmount * tokenPrice;

        // Check if remaining collateral would be sufficient
        require(
            newCollateralValue * simpleLending.COLLATERAL_RATIO() >= borrowedValue * 100,
            "Withdrawal would exceed collateral ratio"
        );
    }

    // Verify that total borrowed tokens never exceed total collateral value
    function assertionBorrowedInvariant() external {
        ph.forkPostState();
        uint256 ethPrice = ethPriceFeed.getPrice();
        uint256 tokenPrice = tokenPriceFeed.getPrice();

        uint256 collateralValue = simpleLending.totalCollateral() * ethPrice;
        uint256 borrowedValue = simpleLending.totalBorrowed() * tokenPrice;

        // Must maintain 75% collateral ratio
        require(
            (collateralValue * simpleLending.COLLATERAL_RATIO()) >= (borrowedValue * 100),
            "Borrowed tokens exceed collateral value"
        );
    }

    // Verify price feeds haven't moved dramatically between states
    function assertionPriceDeviation() external {
        uint256 MAX_PRICE_DEVIATION = 10; // 10%
        ph.forkPreState();
        uint256 preTokenPrice = tokenPriceFeed.getPrice();

        ph.forkPostState();
        uint256 postTokenPrice = tokenPriceFeed.getPrice();

        // Check token price deviation
        uint256 tokenDeviation = (
            (postTokenPrice > preTokenPrice) ? postTokenPrice - preTokenPrice : preTokenPrice - postTokenPrice
        ) * 100 / preTokenPrice;

        // Deviation should be less than 10%
        require(tokenDeviation <= MAX_PRICE_DEVIATION, "Token price deviation too high");
    }

    // Prevent large sudden drops in total collateral
    function assertionEthDrain() external {
        uint256 MAX_WITHDRAWAL_PERCENT = 50; // 50%
        ph.forkPreState();
        uint256 preTotalCollateral = simpleLending.totalCollateral();

        ph.forkPostState();
        uint256 postTotalCollateral = simpleLending.totalCollateral();

        if (postTotalCollateral >= preTotalCollateral) {
            return;
        }

        uint256 withdrawalPercent = ((preTotalCollateral - postTotalCollateral) * 100) / preTotalCollateral;
        require(withdrawalPercent <= MAX_WITHDRAWAL_PERCENT, "Withdrawal percentage too high");
    }
}
