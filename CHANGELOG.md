# Pex 1.2.4

- The Owned and HD badges now distinguish films in the same series and remakes with different release years. Pex still recognises punctuation differences, release dates a year apart for the same full title, and an omitted subtitle when the year agrees and the library match is unique. Existing installations rebuild the owned cache on first launch.
- A one-shot recording marks its selected airtime. Later repeats remain available to browse.
- Guide and library refreshes include committed SQLite write-ahead log changes. A failed copy keeps the last working mirror and can be retried. Manual library refresh runs in the background.
- Rebuilding the poster cache clears the artwork directory and preserves owned-film dates. Repeated airings share one poster download. Listings also open with an empty poster cache when prefetch is disabled.
- Windows cache paths support UNC shares. The sample configuration can be copied directly, and the app honours its logging level.
- Windows and Linux releases use a shared Cargo lockfile and verify the package version and executable platform before packaging.
