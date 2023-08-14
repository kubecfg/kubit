# Kubit

## The operatorless operator

### Usage

- To install the operator, run `kubectl apply -k https://github.com/kubecfg/kubit//kustomize/global?ref=v0.0.5`
- Install an `AppInstance` `CustomResource` that provides an OCI package and any necessary configuration
- Watch as the operator pulls the package, and applies the configuration, deploying your services/etc

**¡WORK IN PROGRESS!**

"Packager" persona (person who makes the package):

1. Package templates in OCI artifact
2. Define template engine and parameters in OCI artifact metadata

"User" personal (person who installs a package)

1. Install the kubit operator once
2. Apply a CR that references a package OCI artifact

Example CR:

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
