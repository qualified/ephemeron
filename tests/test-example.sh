#!/bin/bash
set -euf -o pipefail

_term() {
  kill -TERM "$controller" 2>/dev/null
}

trap _term SIGTERM

echo "::group::Add CRD and wait until accepted"
kubectl apply -f k8s/ephemerons.yaml
kubectl wait --for=condition=NamesAccepted crd/ephemerons.qualified.io
echo "::endgroup::"

echo "::group::Waiting for LoadBalancer to get external ip"
ip=""
while [ -z $ip ]; do
  ip=$(kubectl get svc -o=jsonpath='{.status.loadBalancer.ingress[0].ip}' -n kube-system traefik || true)
  [ -z "$ip" ] && sleep 2
done
echo "::endgroup::"

echo "::group::Run Controller"
export EPHEMERON_DOMAIN="$ip.sslip.io"
cargo run &
controller=$!
echo "::endgroup::"

echo "::group::Create example and wait until Available"
export EXPIRES=$(date -d "+1 days" -Iseconds --utc)
envsubst < k8s/example.yaml | kubectl apply -f -
kubectl wait --for=condition=Available --timeout=60s ephemeron/example
echo "::endgroup::"


echo "::group::Check nginx default page"
host=$(kubectl get eph example -o jsonpath='{.metadata.annotations.host}')
curl "$host" | grep "<h1>Welcome to nginx!</h1>"
echo "::endgroup::"


echo "::group::Clean up"
kubectl delete -f k8s/example.yaml
kill -TERM "$controller" 2>/dev/null
echo "::endgroup::"
