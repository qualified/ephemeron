api-image:
	docker buildx build --tag qualified/ephemeron-api:latest --file ./k8s/api/Dockerfile .
.PHONY: api-image

controller-image:
	docker buildx build --tag qualified/ephemeron-controller:latest --file ./k8s/controller/Dockerfile .
.PHONY: controller-image
