# ephemeron

[![Project Status: Concept – Minimal or no implementation has been done yet, or the repository is only intended to be a limited example, demo, or proof-of-concept.](https://www.repostatus.org/badges/latest/concept.svg)](https://www.repostatus.org/#concept)
[![CI](https://github.com/kazk/ephemeron/workflows/CI/badge.svg)](https://github.com/kazk/ephemeron/actions?query=workflow%3ACI)


Kubernetes operator for ephemeral service with auto-ingress.

```yaml
kind: Ephemeron
apiVersion: ephemerons.qualified.io/v1alpha1
metadata:
  name: "foo"
spec:
  # The name of the image to use.
  image: "nginx"
  # The exposed port to route to.
  port: 80
  # When to kill
  expires: "2021-03-01T00:00:00Z"
```

With `EPHEMERON_DOMAIN=example.com`, creating the above resource makes the service available at `foo.example.com` until `2021-03-01T00:00:00Z`.

## Configurations

The controller is configured with the following environment variables:

- `EPHEMERON_DOMAIN` (required): The main domain to use.

## Status Condition Types

- `PodReady`: `True` when `Pod` is `Ready` (not necessarily serving).
- `Available`: `True` when `Service` has endpoints associated.

## Project Structure

```text
.
├── k8s/                 Kubernetes manifests
│   ├── ephemerons.yaml  - Ephemeron CRD
│   └── example.yaml     - Example resource
└── src/
    ├── api/             Implements the Web API
    ├── bin/             Executables
    │   ├── api.rs       - Start Web API
    │   ├── crd.rs       - Output CRD YAML
    │   └── run.rs       - Run controller (default-run)
    ├── controller/      Implements the Controller
    ├── resource/        Implements the Custom Resource
    └── lib.rs
```

**Dev Commands**

- `cargo run`: Run controller
- `cargo run --bin crd`: Output CRD
- `cargo run --bin api`: Start Web API server

## Usage 
### Run Controller

Add CRD and wait for `NamesAccepted` condition:
```bash
kubectl apply -f k8s/ephemerons.yaml
kubectl wait --for=condition=NamesAccepted crd/ephemerons.qualified.io
```

Run controller:
```bash
EPHEMERON_DOMAIN=example.com cargo run
```

<details>
<summary><a href="http://sslip.io">sslip.io</a> can be used for local development</summary>

`k3d/k3s` example:
```bash
LB_IP=$(kubectl get svc -o=jsonpath='{.status.loadBalancer.ingress[0].ip}' -n kube-system traefik)
EPHEMERON_DOMAIN="$LB_IP.sslip.io" cargo run
```

> `*.10.0.0.1.sslip.io` resolves to `10.0.0.1`

</details>

### With `kubectl`

Add `Ephemeron`:

```bash
# Set environment variable `EXPIRES` and apply `k8s/example.yaml` with it.
# The following example will expire tomorrow.
export EXPIRES=$(date -d "+1 days" -Iseconds --utc)
envsubst < k8s/example.yaml | kubectl apply -f -
# Wait for the `Available` condition
kubectl wait --for=condition=Available ephemeron/example
```

Check that the example is deployed:
```bash
host=$(kubectl get eph example -o jsonpath='{.metadata.annotations.host}')
curl $host | grep "<h1>Welcome to nginx!</h1>"
```

### Web API

> DO NOT EXPOSE THIS TO PUBLIC

<details>
<summary>Routes</summary>

- `POST /`: Create new resource
  - Payload is `EphemeronSpec`
  - Responds with `{"id": unique_id}`. Use this `id` to control the resource.
- `GET /{id}`: Get the hostname
  - Responds with `{"host": hostname}`.
  - `hostname` is a string `{id}.{domain}` when available. Otherwise, `null`.
- `DELETE /{id}`: Delete the resource and any resources it owns.

</details>

Start the server:

```bash
cargo run --bin api
```

Create some service:
```bash
curl \
    -X POST \
    http://localhost:3030/ \
    -H 'Content-Type: application/json' \
    -d "{\"image\": \"nginx\", \"port\": 80, \"expires\": \"$(date -d "+30 minutes" -Iseconds --utc)\"}"
# {"id": "c0nddh7s3ok4clog56n0"}
```

Get the host. (There's no convenient way to wait until it's ready at the moment and `host` is `null` when it's not ready.)
```bash
curl localhost:3030/c0nddh7s3ok4clog56n0
# {"host": "c0nddh7s3ok4clog56n0.example.com"}
```

See if it's working:
```bash
curl c0nddh7s3ok4clog56n0.example.com | grep "<h1>Welcome to nginx!</h1>"
# <h1>Welcome to nginx!</h1>
```

## Deploying

### k3d

#### Push images to a local registry

```bash
# Create local registry first
k3d registry create dev.localhost
# Find the port
PORT=$(docker port k3d-dev.localhost 5000/tcp | cut -d ':' -f 2)
# Create a new cluster with the registry
k3d cluster create dev --registry-use k3d-dev.localhost:$PORT
```

```bash
docker buildx build --tag qualified/ephemeron-controller:latest --file ./k8s/controller/Dockerfile .
docker tag qualified/ephemeron-controller:latest k3d-dev.localhost:$PORT/ephemeron-controller:latest
docker push k3d-dev.localhost:$PORT/ephemeron-controller:latest
```

```bash
docker buildx build --tag qualified/ephemeron-api:latest --file ./k8s/api/Dockerfile .
docker tag qualified/ephemeron-api:latest k3d-dev.localhost:$PORT/ephemeron-api:latest
docker push k3d-dev.localhost:$PORT/ephemeron-api:latest
```
#### Create Service Accounts

```bash
kubectl apply -f k8s/controller/sa.yaml
kubectl apply -f k8s/api/sa.yaml
```

#### Create Deployments

```bash
LB_IP="$(kubectl get svc -o=jsonpath='{.status.loadBalancer.ingress[0].ip}' -n kube-system traefik)"
export DOMAIN="LB_IP.sslip.io"
export IMAGE=k3d-dev.localhost:$PORT/ephemeron-controller:latest
envsubst < k8s/controller/deployment.yaml | kubectl apply -f -

export IMAGE=k3d-dev.localhost:$PORT/ephemeron-api:latest
export HOST="api.$DOMAIN"
envsubst < k8s/api/deployment.yaml | kubectl apply -f -
```

## Cleaning Up

Delete all `Ephemeron`s. All the resources owned by them are deleted as well:
```bash
kubectl delete ephs --all
```

## References

- Ingress: [Name based virtual hosting](https://kubernetes.io/docs/concepts/services-networking/ingress/#name-based-virtual-hosting)
