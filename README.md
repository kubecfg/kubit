# Kubit

## The kubecfg operator

The `kubit` operator is a Kubernetes controller that can render and apply jsonnet templates based on the [kubecfg](https://github.com/kubecfg/kubecfg) jsonnet tooling/framework.


## Installation

### Kubernetes controller

```bash
kubectl apply -k https://github.com/kubecfg/kubit//kustomize/global?ref=v0.0.6
```

The Kubernetes controller is the main way to use kubit.

### CLI tool

#### Homebrew

(popular on macos, but also available on linux)

```bash
brew install kubecfg/kubit/kubit
```

#### cargo install

Direct install from sources:

```bash
cargo install --git https://github.com/kubecfg/kubit/ --tag v0.0.6
```

The CLI is an optional tool that provides helpers and alternative ways to install and inspect packages.

## Usage

### Install a package

1. Install the kubit operator once
2. Apply a CR that references a package OCI artifact

Example `foo.yaml` CR:

```yaml
apiVersion: kubecfg.dev/v1alpha1
kind: AppInstance
metadata:
  name: foo
  namespace: myns
spec:
  package:
    image: ghcr.io/kubecfg/demo:v0.1.0
    apiVersion: demo/v1alpha1
    spec:
      bar: baz
```

Such a CR can be applied using standard Kubernetes tooling such as [kubectl](https://kubernetes.io/docs/tasks/tools/#kubectl),
or [ArgoCD](https://argoproj.github.io/cd/):

```bash
kubectl apply -f foo.yaml
```

### Observe an application instance

The controller will continuously attempt to reconcile the desired state of the application instance
and update the outcome of the reconciliation in the `status` field of the `AppInstance` custom resource.

You can observe the `status` field of the `AppInstance` resource using standard Kubernetes tooling such as:

```bash
kubectl get -f foo.yaml -o json | jq .status
```

TIP: render logs in more readable format with:

```bash
kubectl get -f foo.yaml -o json | jq -r '.status.lastLogs|to_entries[] | "\(.key): \(.value)"'
```

### Creating a new package

The `kubecfg pack` command can be used to take a jsonnet file and all its dependencies and push them
all together as a bundle into an OCI artifact.

```bash
kubecfg pack ghcr.io/kubecfg/demo:v0.1.0 demo.jsonnet
```

### Installing packages manually

You can run the same logic that the kubit controller does when rendering and applying a template by running
the `kubit` CLI tool from your laptop:

```bash
kubit local apply foo.yaml
```

Kubit is just a relatively thin wrapper on top of `kubecfg`.
For increased compatibility, it uses standard `kubectl apply` to apply the manifests using more standard
tooling rather than kubecfg's integrated k8s API.

You can preview the actuall commands that `kubit` will run with:

```bash
kubit local apply foo.yaml --dry-run=script
```

Other interesting options are `--dry-run=render` and `--dry-run=diff` which will respectively just render the YAML without applying it
and rendering + diffing the manifests against a running application. This can be useful to preview effects of changes in the spec or
between versions of a package