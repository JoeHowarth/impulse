# Bevy Development Notes

## Understanding the Bevy API

The Bevy game engine is included as a local submodule in this project. When working with Bevy APIs or looking for usage examples, you can reference the examples in the `bevy/examples/` directory.

These examples demonstrate the current Bevy API patterns and best practices for the version we're using (0.17.3).

## Local Development

This project uses a local Bevy dependency via git submodule, allowing us to:
- Make custom modifications to Bevy if needed
- Reference example code directly
- Ensure all dependencies use the same Bevy version via `[patch.crates-io]`
