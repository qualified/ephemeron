# prefixing stage name with `api-` because `buildx` gets confused with the controller one
FROM clux/muslrust:stable AS api-planner
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json


FROM clux/muslrust:stable AS api-cacher
RUN cargo install cargo-chef
COPY --from=api-planner /volume/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json


FROM clux/muslrust:stable AS api-builder
COPY . .
COPY --from=api-cacher /volume/target target
COPY --from=api-cacher /root/.cargo /root/.cargo
RUN cargo build --bin api --release --target x86_64-unknown-linux-musl


FROM gcr.io/distroless/static:nonroot
COPY --from=api-builder --chown=nonroot:nonroot /volume/target/x86_64-unknown-linux-musl/release/api /app/api
ENTRYPOINT ["/app/api"]
