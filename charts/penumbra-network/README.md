# penumbra-network Helm chart

Deploys a [Penumbra] network via [Helm].

The chart will first generate a network config, complete with genesis,
and then instantiate genesis validators based on that config and run them.
By default, two (2) genesis validators will be created, but this is configurable
via the `pd` options.

[Penumbra Labs]: https://penumbralabs.xyz
[Helm]: https://helm.sh/
