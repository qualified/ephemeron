api-image:
	docker buildx build --tag ghcr.io/qualified/ephemeron-api:latest --file ./k8s/api/Dockerfile .
.PHONY: api-image

controller-image:
	docker buildx build --tag ghcr.io/qualified/ephemeron-controller:latest --file ./k8s/controller/Dockerfile .
.PHONY: controller-image
