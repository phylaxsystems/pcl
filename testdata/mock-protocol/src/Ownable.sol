// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

contract Ownable {
    address private _owner;

    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    constructor() {
        _owner = address(0xdead);
        emit OwnershipTransferred(address(0), _owner);
    }

    modifier onlyOwner() {
        require(_owner == msg.sender, "Ownable: caller is not the owner");
        _;
    }

    // Get the current owner
    function owner() public view returns (address) {
        return _owner;
    }

    // It's very unlikely that the owner should change
    // Governance updates like owner change should be planned well ahead of time
    // and the assertions can be paused with a cooldown period when this is planned
    // We can define an assertion that checks if the owner changes
    function transferOwnership(address newOwner) public onlyOwner {
        require(newOwner != address(0), "Ownable: new owner is the zero address");
        emit OwnershipTransferred(_owner, newOwner);
        _owner = newOwner;
    }
}
