# Test fixtures

`test-licence-key.pem` is a **test-only** RSA private key, committed on purpose.

It signs licence keys inside the test suite so verification can be exercised end to end.
It is deliberately *not* the key the binary verifies against: `a_key_signed_by_another_issuer_is_refused` signs with this key and checks it against the real public key, and that test fails if the two are ever made the same.

Nothing signed with this key is accepted by a released binary.
The real signing key is not in this repository.
