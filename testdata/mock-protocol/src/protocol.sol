// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

contract MockProtocol {

    uint example;

    constructor() {
        example = 1;
    }

  function increment() public {
    example++;
  }
}
