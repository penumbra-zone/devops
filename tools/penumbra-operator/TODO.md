# goals

At a high level, I want to deploy `n` PenumbraNodes
and have everything wired up correctly:

  1. public ip address
  2. chain-id/network/bootstrap-url (sane defaults)
  3. persistent_peers? (nah, family-management can wait)

# brainstorm
Do we need an operator pattern for this? Kind of stumped
about how verbose it is to re-declare the Helm resources
as Rust structs. Maybe it'd be better to `include!` the
yaml manifests and customize them after import?
Change them to tera templates?

# `penumbra-deployer` control flow

1. generate release-name uuid
2. request lbs first, wait
3. pass lbs to rest of chart

Should we just script this? Required args would be:

  * --moniker (mandatory; optional if we autogen)
  * --bootstrap-url (optional; sane default exists)

Then we can auto-default everything else. That means resources to deploy would be:

  * StatefulSet
  * Services
  * ServiceAccount (necessary? I think not actually)

# todo

- [ ] add support for creating nodes
  - [x] deploy statefulset as defined via rust code
  - [x] support archive urls for historical data
  - [x] make sure configmaps are updated on the fly
  - [ ] implement cleanup of resources
  - [x] statefulset deletion
  - [x] service deletion
  - [ ] pvc deletion
  - [ ] purge/destroy functionality (clean up pvc only on purge)
  - [ ] p2p lb
  - [ ] annotate with chain id
  - [ ] annotate with latest block height
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
  - [ ] penumbra-operator crdgen # port from existing bin
  - [ ] penumbra-operator install # uses crdgen
