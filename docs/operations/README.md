# Operations

Operational documents for running and maintaining a `nas-csi` managed cluster.

- [Observability](observability.md)
- [Runbooks](runbooks/README.md)
- [First deploy](runbooks/first-deploy.md)
- [Rollback](runbooks/rollback.md)
- [Workload validation](runbooks/workload-validation.md)
- [Node maintenance](runbooks/node-maintenance.md)
- [Cluster rebuild](runbooks/cluster-rebuild.md)

These runbooks are intentionally conservative. Shared TrueNAS datasets are
authoritative storage and must not be deleted as part of VM or cluster rebuilds.
