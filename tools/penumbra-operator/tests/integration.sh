#!/usr/bin/env bash
# Quick and dirty integration testing for penumbra-operator.
# Runs destructive actions against a real cluster!


set -euo pipefail
set -x

kubectl -n penumbra get penumbranetworks -o name \
  | xargs -r kubectl -n penumbra delete --wait
kubectl -n penumbra get pvc -l app.kubernetes.io/component=genesis-validator -o name \
  | xargs -r kubectl -n penumbra delete
sleep 10
kubectl apply -f files/crd-network-example.yaml




# Things to test:
#
# - [ ] services have endpoints
# - [ ] dns resolves in a multi-validator setup
# - [ ] 
