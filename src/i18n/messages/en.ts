import { wordmark } from '../wordmark.ts';

/**
 * English, the source catalogue: every other catalogue has exactly these
 * keys (`satisfies Messages`, checked again when the site builds). Every
 * string a visitor can see lives in a catalogue, never in markup.
 */
const en = {
  /** The language's own name, as the language switch shows it. */
  language: { name: 'English' },
  site: {
    name: wordmark,
    skipToContent: 'Skip to content',
    languageSwitch: 'Language',
  },
  /** One entry per page of `src/lib/pages.ts`: its title and description. */
  pages: {
    home: {
      title: wordmark,
      description: 'A Chrome extension for comparing and reviewing code.',
      heading: wordmark,
      lede: 'A Chrome extension for comparing and reviewing code.',
      placeholder: 'This site is a placeholder. There is nothing to read here yet.',
    },
  },
  notFound: {
    title: 'Page not found',
    description: 'There is no page at this address.',
    heading: 'Page not found',
    message: 'There is no page at this address.',
    home: 'Go to the home page',
  },
};

export default en;

/** The shape every catalogue has: English's keys, any strings. */
export type Messages = typeof en;
