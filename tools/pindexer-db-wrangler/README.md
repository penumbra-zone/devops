# pindexer-db-wrangler

A tool for running [pindexer] at specific versions, against specific
databases, and storing the resulting dbs as local dumps.

## Motivation
Intended to facilitate review on pending changes for pindexer,
as well as to aid in releasing changes, since pindexer will pause block ingestion
if schema versions have changed. This pause can be 1-3h on mainnet.
Operators mindful of uptime can instead use this tool to prepare a dump,
then restore that dump on top of a remote db.

## Getting started

Copy the `.envrc.example` to `.envrc` locally, in order to activate the nix devshell
with appropriate `pindexer` versions.

```bash
cp .envrc.example .envrc
```

Then, edit the `.envrc` file to provide accurate database URLs for dumping the source db.
The raw cometbft db will be imported locally, in an ephemeral postgres instance using
unix domain sockets, and `pindexer` will run against that source. After completion,
a `pindexer.dump` file will be created in the project dir.
