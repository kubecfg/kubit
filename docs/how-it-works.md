# How does `kubit` work?

## Overview

Kubit handles the reconcillation process of an `AppInstance` through a Kubernetes
`Job`. This job runs various `kubecfg` and `kubectl` commands. The artifact
which an `AppInstance` points to is expected to be an OCI bundle for these commands
to interpret.

At a high-level these jobs are split between rendering and applying:

1. The `initContainers` of a `Job` handle the retrieval of the current `AppInstance`
and using it as an overlay to `kubecfg show`.
2. The output of the `kubecfg show` is written to an `/overlay` directory for the
main `Job` container. This is marks the completion of the render step.
3. The apply step uses `kubectl`[^1] to create the Kubernetes resources which were
contained within the `AppInstance` bundle.

[^1]: Currently, this requires `KUBECTL_APPLYSET=true` as it is an alpha feature.

The tracking of resources generated from an `AppInstance` is handled through an
[ApplySet][k8s-applyset]. This set can be used to prune objects which are not
part of the set and is also used to uninstall the resources created by an `AppInstance`.


### Future work

Currently, `kubit` only supports [jsonnet][jsonnet] as an installation engine,
but future work should involve the introduction of other engines.


[jsonnet]: https://jsonnet.org/
[k8s-applyset]: https://kubernetes.io/blog/2023/05/09/introducing-kubectl-applyset-pruning/
