# kartta-languages

Every language the kartta code graph ships, as one set to build an engine with, or a selection of them by name.

kartta's engine names no language of its own: `Engine::build` takes the languages to build with, each a value
implementing `kartta_lang::Language`, and kartta's own sixteen live in fourteen language crates, `kartta-lang-jsts`
holding three. `all()` gives every one of them, and `select(names)` gives the ones a list names, for a consumer that
takes the list from its own caller, so neither has to name those crates.

| Name         | Crate                 | Files                                                                    |
|--------------|-----------------------|--------------------------------------------------------------------------|
| `go`         | `kartta-lang-go`      | `.go`                                                                    |
| `cpp`        | `kartta-lang-cpp`     | `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`, `.inl`, `.inc` |
| `typescript` | `kartta-lang-jsts`    | `.ts`, `.mts`, `.cts`                                                    |
| `tsx`        | `kartta-lang-jsts`    | `.tsx`                                                                   |
| `javascript` | `kartta-lang-jsts`    | `.js`, `.jsx`, `.mjs`, `.cjs`                                            |
| `daslang`    | `kartta-lang-daslang` | `.das`                                                                   |
| `quirrel`    | `kartta-lang-quirrel` | `.nut`                                                                   |
| `blk`        | `kartta-lang-blk`     | `.blk`                                                                   |
| `rust`       | `kartta-lang-rust`    | `.rs`                                                                    |
| `python`     | `kartta-lang-python`  | `.py`, `.pyi`                                                            |
| `bash`       | `kartta-lang-bash`    | `.sh`, `.bash`                                                           |
| `php`        | `kartta-lang-php`     | `.php`, `.phtml`                                                         |
| `ruby`       | `kartta-lang-ruby`    | `.rb`                                                                    |
| `lua`        | `kartta-lang-lua`     | `.lua`                                                                   |
| `puppet`     | `kartta-lang-puppet`  | `.pp`                                                                    |
| `dshl`       | `kartta-lang-dshl`    | `.dshl`                                                                  |

`NAMES` lists the names in this order, which is the order `all()` gives the languages in.

## Order

Each language claims its files by extension, in any case, and no two claim the same file. The engine hands a file to
the first language that claims it, so the order decides only the order the engine lists the graphs and counts of the
languages in, and that a consumer's own language put after them gets only the files none of them claims.

## Selection

`select` takes the names as `NAMES` spells them, exactly, and gives their languages in the order of `NAMES`, each once
however often it is named. A name that is not one of `NAMES` fails the whole selection with `UnknownLanguages`, whose
message names every such name and lists `NAMES`. No name selects no language, which builds an engine that indexes
the tree and holds no graph.

TypeScript, TSX and JavaScript are three languages of one family, `jsts`, and each is selected by its own name. Naming
`typescript` without `tsx` builds the `.ts`, `.mts` and `.cts` files into the family's graph and leaves the `.tsx` files
out of every graph: the index still holds them, but nothing they define is in a graph, so a call from a `.ts` file into
a component of one binds to nothing and counts as unresolved. Naming any other part of the family leaves the rest out
the same way.

Every language is a value without state, so the set and a selection allocate only the vector that holds them. Every
grammar is compiled into a consumer of this crate whatever it selects; one that wants fewer grammars in its build
depends on the language crates it wants instead.
