# Examples

**demo.daw.json** — an 8-track, 36-bar electro groove composed entirely
through the MCP server (an LLM wrote every pattern, arranged the song, and
mixed it with the listening tools). Self-contained: needs only the free
soundfont from `../fetch-assets.sh`.

```bash
cargo run --release -- --gui examples/demo.daw.json
```
