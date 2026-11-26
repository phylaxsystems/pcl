// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

contract Guard {
    function policy() external pure returns (uint256) {
        return 999;
    }
}
