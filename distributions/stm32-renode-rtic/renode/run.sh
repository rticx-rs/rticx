script_dir=$(dirname "$(realpath "$0")")
cd $script_dir
cd ..
# cargo build --bin core1; cargo build --bin core2
# cargo build --example hello_rtic
cargo microamp --example ping_pong
cd -
renode renode-config.resc