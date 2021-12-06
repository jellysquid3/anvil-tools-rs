anvil-tools-rs
====

:warning: **This tool might damage your world file!** Not a lot of testing has been done here, but my preliminary
examination seems to suggest it at least works for my use case.

A small collection of tools (well, really only *one* for now) to deal with Minecraft's Anvil chunk storage system while
out of the game. Might be useful for server operators or people who do not want to wait on Minecraft's "world optimization..."

## Features
- Strip cached world data (similar to the "Optimize World" feature in vanilla)
- Compress region files for long-term archival (2-3x improvement in compression ratio when compared to tarring the region directory)

## Usage

Use the `--help` argument for usage information.

## Why?

Minecraft's built-in tools have a few issues that occasionally bite me when debugging issues. In no particular order,
the vanilla tools...

- ... are very slow, taking multiple hours on modest worlds.
- ... often crash due to concurrency issues and heap exhaustion from unbounded queues.
- ... only support in-place modification, making the previous point more painful.
- ... require you to have either the Minecraft client or server running.
