# Grand archive

A collection of scripts for maintaining a collection of historical artifacts relevant to the
[Penumbra] network. These include snapshots of node state, in the form of gzipped tarballs,
as well as [reindexer] archives, and genesis files at each upgrade boundary.

## Motivation

In order for new nodes to [join a network](https://guide.penumbra.zone/network/node/pd/join-network),
the node operator must first obtain a record of recent node state, containing both the `pd/rocksdb/` and `cometbdt/data/` directories.
A variety of community-maintained options exist, run by RPC providers and validators.

However, if protocol changes are made to the project that affect the emission of [ABCI events](https://docs.cosmos.network/v0.45/core/events.html),
then full knowledge of all blocks behind historical upgrade boundaries is required for tools like the [reindexer] to work.
Additionally, any service provider wishing to run a [pindexer] database as a backend for web applications will
need to bootstrap that database from somewhere, such as historical reindexer sqlite3 databases.

The grand archive provides those access to all of those artifacts.

## Getting started

Copy the `.envrc.example` to `.envrc` locally, in order to activate the [nix] devshell with appropriate tool versions.
Or you can install tools like the `aws` CLI manually.

Then, to copy the assets to object storage that you control, use a command like this:

```
# Copy from PL DigitalOcean to Cloudflare
aws s3 sync s3://penumbra-labs-artifact-storage/ \
   s3://penumbra-artifacts-grand-archive/ \
  --endpoint-url https://nyc3.digitaloceanspaces.com \
  --acl public-read \
  --destination-endpoint-url https://<account-id>.r2.cloudflarestorage.com
```

Note that the `--endpoint-url` and `--destination-endpoint-url` flags must match the object storage providers
used for the source and destination archives, respectively.

## Known examples

The original object storage for Penumbra node archives was located at https://penumbra-labs-artifact-storage.nyc3.digitaloceanspaces.com/.
Other parties may choose to replicate the contents of that repository and host themselves.

## What's with the name?

It's a playful reference to the [Grand Archive megastructure](https://stellaris.paradoxwikis.com/Grand_Archive_(DLC))
in the videogame Stellaris, a game near and dear to the hearts of many Penumbra devs. In the game, the Grand Archive
allows a civilization to exhibit the treasures it has collected. Hence, it's a suitable name for a repository of
historical network state.

[Penumbra]: https://penumbra.zone
[reindexer]: https://github.com/penumbra-zone/reindexer
[nix]: https://nixos.org/download/
[pindexer]: https://guide.penumbra.zone/network/event-indexing
