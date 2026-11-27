/**
 * The extension builds the site may hand a licence to: their ids, from
 * DEVTAB_EXTENSION_IDS (comma-separated; read when the server runs, public
 * values). An id is 32 letters a–p, as Chrome makes them. Only an extension
 * whose manifest lists this site in `externally_connectable` can hear the
 * site at all; the list keeps the page from talking to any other.
 */
export function extensionIds(): string[] {
  return (process.env.DEVTAB_EXTENSION_IDS ?? '')
    .split(',')
    .map(id => id.trim())
    .filter(id => /^[a-p]{32}$/.test(id));
}
