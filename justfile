phoundry-repo := "https://github.com/phylaxsystems/phoundry"
phoundry-dir := "phoundry"

setup-phoundry mode="release":
    #!/usr/bin/env sh
    cd {{phoundry-dir}} && cargo build --bin forge --{{mode}}

build-all mode="release":
    cargo build --{{mode}}
    just update-phoundry
    just setup-phoundry mode={{mode}}

update-phoundry:
    git submodule update --init --recursive --remote
    cd {{phoundry-dir}} && git checkout master && git pull

place-phoundry-bin mode="release":
    mkdir -p target/{{mode}}
    cp {{phoundry-dir}}/target/{{mode}}/forge target/{{mode}}/phorge

    
