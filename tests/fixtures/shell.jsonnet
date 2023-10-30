// Example jsonnet used for a package.
{
  service: {
    apiVersion: 'v1',
    kind: 'Service',
    metadata: {
      name: 'shell',
    },
    spec: {
      clusterIP: 'None',
      selector: {
        app: 'shell',
      },
    },
  },
  sts: {
    apiVersion: 'apps/v1',
    kind: 'StatefulSet',
    metadata: {
      name: 'shell',
    },
    spec: {
      minReadySeconds: 30,
      replicas: 1,
      selector: {
        matchLabels: {
          app: 'shell',
        },
      },
      serviceName: 'shell',
      template: {
        metadata: {
          labels: {
            app: 'shell',
          },
        },
        spec: {
          containers: [
            {
              command: [
                'bash',
                '-c',
                'set -e\nsleep 2\napt-get update\napt-get install -y curl wget\nsleep 1000002\n',
              ],
              image: 'debian:11',
              name: 'shell',
              securityContext: {
                privileged: false,
              },
            },
          ],
          terminationGracePeriodSeconds: 2,
        },
      },
    },
  },
}
