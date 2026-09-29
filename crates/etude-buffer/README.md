# etude-buffer

> Copy-avoiding byte reader/writer buffer traits.

This crate defines the chunk-oriented `reader::Buffer` and `writer::Buffer` traits used to
move bytes between buffers while avoiding copies wherever possible, along with a family of
adapters (`chain`, `tracked`, `io_slice`, …) and, under the `testing` feature, a stream-data
model and generators for property/fuzz tests.

The traits are cursor-free: anything that tracks offsets (stream position, final offset)
layers that on top downstream.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-buffer).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
