/**
 * Every localised page of the site, by id, with its locale-neutral path
 * (`''` is the home page). The page itself is `src/pages/[...locale]/<path>`;
 * its title and description are `pages.<id>` in every catalogue. The
 * language switch, the alternates, the sitemap and the tests read this list,
 * so a page missing here is a page the tests report as unlisted.
 */
export const pages = {
  home: '',
} as const;

export type PageId = keyof typeof pages;
