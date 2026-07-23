// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import "../src/NoArgsAssertion.a.sol";

interface VmEx {
    function assertion(address adopter, bytes calldata createData, bytes4 fnSelector) external;
}

contract AssertionAdopter {
    function touch() external pure returns (bool) {
        return true;
    }
}

contract NoArgsAssertionTest {
    VmEx private constant CL =
        VmEx(address(uint160(uint256(keccak256("hevm cheat code")))));

    function testAssertionCheckBool() public {
        NoArgsAssertion assertion = new NoArgsAssertion();
        require(assertion.assertionCheckBool(), "expected assertion to pass");
    }

    function testAssertionRunsThroughPhoundry() public {
        AssertionAdopter adopter = new AssertionAdopter();
        CL.assertion(
            address(adopter),
            type(NoArgsAssertion).creationCode,
            NoArgsAssertion.assertionCheckBool.selector
        );
        require(adopter.touch(), "expected adopter call to pass");
    }
}
