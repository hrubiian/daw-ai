# Examples

**quickstart.daw.json** is *First Light*, an eight-bar groove with drums, bass,
and electric piano. It uses the default General MIDI SoundFont and the built-in
engine. Follow [Quickstart](../README.md#quickstart) to set up the SoundFont.

```bash
cargo run --release --locked -- --gui examples/quickstart.daw.json
```

**demo.daw.json** is the original 36-bar electro groove composed through MCP.
It uses VCSL and additional WAV samples. Run
`cargo run --release --locked -- doctor --headless examples/demo.daw.json`
to check which files are missing from your checkout.
