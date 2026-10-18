# kartta-ffi

C ABI of the kartta code graph engine: a handle on a tree, one JSON call per query, and the frees.

The static library exports the five functions [`include/kartta.h`](include/kartta.h) declares. A method is a name and
a JSON object of params, and every answer is `{"ok": <answer>}` or `{"err": {"code": <code>, "message": <message>}}`,
so a binding translates JSON and decides nothing. The Go binding in `bindings/go` links the static library; a Rust shell
builds on the `Kartta` type of the rlib.

Each method's contract is a file in [`tests/golden`](tests/golden): a request on the fixture tree beside it, and the
answer it gets.

```c
KarttaString error;
KarttaHandle *handle = kartta_open((const uint8_t *)"{\"root\":\".\"}", 12, &error);
KarttaString built = kartta_call(handle, "build", NULL, 0);
kartta_string_free(built);
KarttaString callers = kartta_call(handle, "callers", (const uint8_t *)"{\"name\":\"main\"}", 15);
kartta_string_free(callers);
kartta_free(handle);
```
