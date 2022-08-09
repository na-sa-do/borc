# borc: CBOR done right

borc is an implementation of [CBOR, the Concise Binary Object Representation](https://cbor.io), as defined in [RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html). CBOR is a simple but powerful data format whose potential has so far gone underused, especially in the Rust space (due to the dominance of Serde, which does not and probably cannot support its full feature set). borc aims to be a one-stop shop for CBOR and CBOR-based protocols.

## Status

At the moment, the basic streaming encoder and decoder are implemented. Being stream-based, they're a bit clunky to use. I plan to write three further interfaces:

- a basic tree-based encoder and decoder
- an extended stream-based encoder and decoder, which can handle CBOR tags
- an extended tree-based encoder and decoder

The following measures will be taken to ensure that the extended implementations can add new extensions while maintaining backwards compatibility:

- The enums used for their data structures will be marked `#[non_exhaustive]`. This permits adding additional variants at any time without breaking clients at compile time.
- New extensions will be optional at compile-time, if they are complex (enough to result in noticeably slower processing) or require additional dependencies.
- New extensions will also be optional on a per-encoder/decoder basis, so that it is safe for all users to simply ignore or use `unreachable!()` to handle extensions they don't care about (since those extensions won't actually show up at runtime).

Theoretically it might be better to pursue a layered implementation, with extensions implemented as filters over lower-level, less capable implementations. This would also allow external crates to implement extensions that borc proper would not. However, I think that would be more verbose, less efficient at runtime, and error-prone (since there would be no way to stop users from accidentally repeating a layer twice, for instance). Of course, anyone can still implement additional extensions as layers in that way.

## License

This library is distributed under the terms of either of:

- the [MIT License](LICENSES/MIT.txt) ([http://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT))
- the [Apache License, Version 2.0](LICENSES/Apache-2.0.txt) ([http://www.apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0))

at your option.

### Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.