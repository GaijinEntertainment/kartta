//! Links the addon the way Node loads it: N-API's symbols resolve against the process that loads the library.

fn main() {
    napi_build::setup();
}
