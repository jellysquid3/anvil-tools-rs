anvil-tools-rs
====

:warning: **This tool might damage your world file!** Not a lot of testing has been done here, but my preliminary
examination seems to suggest it at least works for my use case.

A small collection of tools (well, really only *one* for now) to deal with Minecraft's Anvil chunk storage system while
out of the game. Might be useful for server operators or people who do not want to wait on Minecraft's "world optimization..."

## Usage

Clone the repository and the submodules...
```
git clone --recursive https://github.com/jellysquid3/anvil-tools-rs
```

Build the project...

```
cargo build --release
```

Run the built binary...

```
./target/release/anvil-tools-rs ...
```

### Commands

#### Strip cached data

Strips any cached data from the region files within `<INPUT>` and rewrites them to `<OUTPUT>`.

```
anvil-tools-rs strip-cached-data <INPUT> <OUTPUT>
```


## Why?

Minecraft's built-in tools have a few issues that occasionally bite me when debugging issues. In no particular order,
the vanilla tools...

- ... are very slow, taking multiple hours on modest worlds.
- ... often crash due to concurrency issues and heap exhaustion from unbounded queues.
- ... only support in-place modification, making the previous point more painful.
- ... require you to have either the Minecraft client or server running.