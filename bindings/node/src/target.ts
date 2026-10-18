/**
 * The target the addon of this host is built for, as `<platform>-<arch>[-<libc>]`: the addon's file is
 * `kartta.<target>.node`, and the package that ships it is `kartta-<target>`. Linux is taken as glibc, the only libc
 * a package ships an addon for.
 */
export function target(): string {
  return `${process.platform}-${process.arch}${process.platform === "linux" ? "-gnu" : ""}`;
}
