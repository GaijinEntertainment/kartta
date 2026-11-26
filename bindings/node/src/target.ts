/** The targets a package ships an addon for: ADR 4.46's support matrix. */
export const TARGETS = [
	'darwin-arm64',
	'linux-arm64-gnu',
	'linux-arm64-musl',
	'linux-x64-gnu',
	'linux-x64-musl',
	'win32-x64-msvc',
] as const;

/** What {@link target} reads of the process. */
export type Host = {
	platform: string;
	arch: string;
	report: {excludeNetwork?: boolean | undefined; getReport: () => unknown};
};

/**
 * The target the addon of this host is built for, as `<platform>-<arch>[-<abi>]`: the addon's file is
 * `kartta.<target>.node`, and the package that ships it is `@gaijin/kartta-<target>`.
 */
export function target(host: Host = process): string {
	const stem = `${host.platform}-${host.arch}`;
	if (host.platform === 'linux') {
		return `${stem}-${linuxAbi(host.report)}`;
	}
	return host.platform === 'win32' ? `${stem}-msvc` : stem;
}

/**
 * The C library this process runs on. Node sets a diagnostic report's `header.glibcVersionRuntime` from the running
 * process's `gnu_get_libc_version`, which glibc alone defines, so its absence means musl
 * (https://nodejs.org/docs/latest-v24.x/api/report.html).
 */
function linuxAbi(report: Host['report']): 'gnu' | 'musl' {
	const {excludeNetwork} = report;
	// constraint: a report otherwise resolves the peer of every open socket, synchronously
	report.excludeNetwork = true;
	// SAFETY: every report holds a header, and its field is declared optional.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	const {header} = report.getReport() as {header: {glibcVersionRuntime?: string}};
	report.excludeNetwork = excludeNetwork;
	return header.glibcVersionRuntime === undefined ? 'musl' : 'gnu';
}
