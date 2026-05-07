# deploy/addons

Substrate add-ons that `cluster-manager` may install after the Kubernetes API is
healthy.

Allowed here:

- `nas-csi` chart installation references;
- metrics-server;
- CNI replacement manifests if flannel is disabled;
- load balancer or ingress infrastructure when treated as cluster substrate.

Not allowed here:

- user applications;
- app databases;
- app ingress routes;
- app secrets.

Application deployment should live in a separate repo or GitOps system.
