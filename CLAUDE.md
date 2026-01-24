# Working Style

- Do NOT change approach from what we discussed without talking to me first
- Use relative paths - we're already in the `impulse` directory (no need for `/Users/jh/personal/impulse/...`)
- If a fix isn't working as expected, discuss options before pivoting to a different solution
- When I ask "why isn't X working", I want to understand the problem, not have you silently switch to approach Y

# Bevy Development Notes

## Understanding the Bevy API

The Bevy game engine is included as a local submodule in this project. When working with Bevy APIs or looking for usage examples, you can reference the examples in the `bevy/examples/` directory.

These examples demonstrate the current Bevy API patterns and best practices for the version we're using (0.17.3).

## Local Development

This project uses a local Bevy dependency via git submodule, allowing us to:
- Make custom modifications to Bevy if needed
- Reference example code directly
- Ensure all dependencies use the same Bevy version via `[patch.crates-io]`
- Do not use cargo run
- You can't run the game yourself - I must do that