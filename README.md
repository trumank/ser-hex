# ser-hex

Serialization tracing and visualization tools.

Attempts to answer the question "where did these bytes come from??" when
examining opaque binary blobs of data.

## ser-hex-tui

```console
cargo run --release --bin ser-hex-tui examples/bson/trace.json
```

![ser-hex-tui BSON trace](https://github.com/user-attachments/assets/4b48b982-9a30-44dd-b55c-c58ef9b86c73)


## trace format

The trace output contains the binary data and a tree of stream actions (Read/Seek/Span).

```json
{
  "data": "DAAAAEhlbGxvIFdvcmxkIQ==",
  "start_index": 0,
  "root": {
    "Span": {
      "name": "pascal_string",
      "actions": [
        {
          "Span": {
            "name": "length",
            "actions": [
              {
                "Read": 4
              }
            ]
          }
        },
        {
          "Read": 12
        }
      ]
    }
  }
}
```

## capturing a trace

There are two methods of capturing trace data from rust:

### rust Tracing instrumentation

Builds spans by listening to `tracing::instrument`'d functions. This results
in accurately nested spans but requires manual annotation of functions.

```rust
let mut input = Cursor::new([1, 2, 3]);
ser_hex::read_incremental("trace.json", &mut input, read)?;

#[tracing::instrument(skip_all)]
fn read<R: Read>(input: &mut R) -> std::io::Result<()> {
    input.read_exact(&mut [0; 3])?;
    Ok(())
}
```


### backtrace captures

The second option is to construct spans from backtrace captures. This does not
require sprinkling instrumentation annotations all over the serialization code,
but can result in lower quality trace data. Since backtraces are captured only
on read/seek events, it's impossible to know how far up the stack control flow
went between reads, which can lead to inaccurately reconstructed spans.

In theory, if stack frame push/pop events could be hooked at the hardware
level or via [emulation](https://www.unicorn-engine.org/), they could be made
accurate, but I have not explored this route yet. Still with this limitation, it
provides useful data with little effort.

```rust
let mut input = Cursor::new([1, 2, 3]);
let mut tracer = ser_hex_tracer::TracerReader::new_options(
    &mut input,
    ser_hex_tracer::TracerOptions { skip_frames: 3 }, // number of top level stack frames to omit from trace
);
let res = read(&mut tracer);
tracer.trace().save("trace.json").unwrap();

fn read<R: Read>(input: &mut R) -> std::io::Result<()> {
    input.read_exact(&mut [0; 3])?;
    Ok(())
}
```

### tracing other streams or non-rust code

It is possible to trace arbitrary native code by hooking the necessary stream
functions and calling the corresponding functions on your `Tracer` object. See
[trace_factorio](examples/trace_factorio) and [trace_drg](examples/trace_drg)
for examples of tracing data streams implemented in C++.
