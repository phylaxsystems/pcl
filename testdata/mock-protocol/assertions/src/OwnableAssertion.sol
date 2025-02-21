// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Assertion} from "../../lib/credible-std/src/Assertion.sol"; // Credible Layer precompiles
import {Ownable} from "../../src/Ownable.sol"; // Ownable contract

contract OwnableAssertion is Assertion {
    Ownable ownable;

    constructor(address ownable_) {
        ownable = Ownable(ownable_); // Define address of Ownable contract
    }

    function triggers() external view override {
        registerCallTrigger(this.assertionOwnershipChange.selector);
    }

    // This function is used to check if the ownership has changed
    // Get the owner of the contract before and after the transaction
    // Return false if the owner has changed, true if it has not
    function assertionOwnershipChange() external {
        ph.forkPreState(); // Fork the pre-state of the transaction
        address preOwner = ownable.owner(); // Get the owner of the contract before the transaction
        ph.forkPostState(); // Fork the post-state of the transaction
        address postOwner = ownable.owner(); // Get the owner of the contract after the transaction
        require(postOwner == preOwner, "Ownership has changed"); // Revert if the owner has changed
    }
}
