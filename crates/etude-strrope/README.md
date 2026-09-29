# etude-strrope

> A UTF-8 string rope: a validated-UTF-8 view over the `etude-bytevec` byte rope.

`StrRope` is a cheaply-clonable, copy-avoiding growable string built on the `etude-bytevec` byte
rope. Where `etude_str::Str` is a *flat* `Bytes`-backed string, `StrRope` is its rope-shaped
sibling: a newtype over `etude_bytevec::Rope<Utf8>`, so it gets the tiered rope's O(1) clone and
O(log n) split/concat, with a UTF-8 invariant layered on top.

## Invariant

The rope's **concatenated** byte content is always valid UTF-8. Individual internal chunks may
fall inside a multi-byte codepoint (chunks arrive from arbitrary syscall/network splits), so the
invariant is over the logical byte stream, not per chunk. Byte-indexed operations (`insert_str`,
`split_off`, `slice`) require their offsets to be char boundaries, which `StrRope` checks before
delegating to the raw byte-offset rope ops.

## Interop

`StrRope::into_bytes` returns the underlying `ByteVec` for **free** (only the zero-size kind
marker is dropped — no copy, no re-validation), and `StrRope::from_utf8` validates a `ByteVec`
back into a `StrRope` (O(n)).

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-strrope).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
