// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Credible} from "../../lib/credible-std/src/Credible.sol";
import {OwnableAssertion} from "../src/OwnableAssertion.sol";
import {Ownable} from "../../src/Ownable.sol";
import {console} from "../../lib/credible-std/lib/forge-std/src/console.sol";
import {AssertionTest} from "../../lib/credible-std/src/AssertionTest.sol";

contract TestOwnableAssertion is AssertionTest, Credible {
    address public newOwner = address(0xdeadbeef);
    bytes[] public assertions;
    address public assertionAdopter;

    function setUp() public {
        assertionAdopter = address(new Ownable());
        vm.deal(Ownable(assertionAdopter).owner(), 1 ether);
    }

    function test_assertionOwnershipChanged() public {
        vm.prank(address(0xdead));

        bytes memory transaction = createTransaction(
            Ownable(assertionAdopter).owner(),
            address(assertionAdopter),
            0,
            abi.encodeWithSelector(Ownable.transferOwnership.selector, newOwner)
        );

        assertions.push(abi.encodePacked(type(OwnableAssertion).creationCode, abi.encode(assertionAdopter)));

        assertEq(phvm.assertionEx(transaction, assertionAdopter, assertions), false); // assert that the ownership has changed
    }

    function test_assertionOwnershipNotChanged() public {
        bytes memory emptyTransaction = createEmptyTransaction();

        assertions.push(abi.encodePacked(type(OwnableAssertion).creationCode, abi.encode(assertionAdopter)));

        assertEq(phvm.assertionEx(emptyTransaction, assertionAdopter, assertions), true); // assert that the ownership has not changed
    }
}
