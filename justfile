phoundry-repo := "https://github.com/phylaxsystems/phoundry"
phoundry-dir := "phoundry"

cargo-mode mode="release":
    if [ {{mode}} == "release" ]; then echo "--release"; else echo ""; fi

setup-phoundry mode="release":
    #!/usr/bin/env sh
    cd {{phoundry-dir}} && cargo build --bin forge `just cargo-mode {{mode}}`

build-all mode="release":
    cargo build `just cargo-mode {{mode}}`
    just update-phoundry
    just setup-phoundry {{mode}}
    just place-phoundry-bin {{mode}}

update-phoundry:
    git submodule update --init --recursive --remote
    cd {{phoundry-dir}}

place-phoundry-bin mode="release":
    cp {{phoundry-dir}}/target/{{mode}}/forge target/{{mode}}/phorge

test:
    cargo test --workspace
