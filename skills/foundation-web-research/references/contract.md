# Web Research Connector v1

Search accepts a bounded query, language hint, safe-search choice, and result limit. It returns stable source IDs, canonical URLs, titles, snippets, and provider ranks.

Read accepts a source ID and byte limit. It returns retrieval time, MIME type, normalized text, content hash, truncation state, and `untrusted_external` trust.

A citation binds a source ID, canonical URL, retrieval time, and exact supported quote or source range. Citation validation must reject text that is absent from the retrieved revision.

The host owns network access, authentication, redirects, private-address blocking, download limits, malware policy, and artifact persistence.
