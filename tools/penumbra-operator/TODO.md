# goals

At a high level, I want to deploy `n` PenumbraNodes
and have everything wired up correctly:

  1. public ip address
  2. chain-id/network/bootstrap-url (sane defaults)
  3. stretch goal: persistent_peers, or family management

# todo

- [ ] add support for creating nodes
  - [x] deploy statefulset as defined via rust code
  - [x] support archive urls for historical data
  - [ ] clean up already downloaded archive
  - [x] make sure configmaps are updated on the fly
  - [x] implement cleanup of resources
  - [x] statefulset deletion
  - [x] service deletion
  - [ ] pvc deletion
  - [ ] purge/destroy functionality (clean up pvc only on purge)
  - [ ] p2p lb
  - [ ] annotate with chain id
  - [ ] annotate with latest block height
  - [x] add readinessprobe
  - [ ] add livenessprobe
  - [x] add "pause"/maintenanceMode
- [ ] add support for indexing
  - [x] create pg container
  - [x] create pg db
  - [x] create pg username
  - [x] apply pg schema
  - [x] configure cometbft pg indexer
  - [x] unconfigure cometbft pg indexer
  - [x] confirm actual blocks landing in db
  - [x] use consolidated pvc for db data
- [ ] add support for reindexing
  - [ ] add reindexer-archive container
  - [ ] auto-fetch reindexer archive via init
  - [ ] auto-follow the old blocks
  - [ ] add reindexer-regen container
- [x] add containerfile
- [ ] shared pvc for archive urls
- [ ] add support for creating networks
  - [x] create pvcs
  - [x] create configmaps
  - [x] mount pvcs
  - [x] mount configmaps
  - [x] generate that network
  - [x] copy that stuff over
  - [ ] implement cleanup/retention
  - [ ] create vals (via PenumbraNode?)
- [ ] add subcommands:
  - [ ] impl `deploy` which creates a penumbranode
- [ ] cli interface:
  - [ ] clap dep
  - [x] penumbra-operator run
  - [ ] penumbra-operator deploy
      - [ ] penumbra-operator deploy --moniker --join-url
      - [ ] penumbra-operator delete --moniker
      - [ ] penumbra-operator create-network --chain-id
  - [ ] penumbra-operator pause <node|network>
  - [ ] penumbra-operator crdgen # port from existing bin
  - [ ] penumbra-operator install # uses crdgen
