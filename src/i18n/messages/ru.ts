/** Russian. Keys are English's (`messages/en.ts`), the strings are Russian. */
import { wordmark } from '../wordmark.ts';
import type { Messages } from './en.ts';

export default {
  language: { name: 'Русский' },
  site: {
    name: wordmark,
    skipToContent: 'Перейти к содержимому',
    languageSwitch: 'Язык',
  },
  pages: {
    home: {
      title: wordmark,
      description: 'Расширение Chrome для сравнения и ревью кода.',
      heading: wordmark,
      lede: 'Расширение Chrome для сравнения и ревью кода.',
      placeholder: 'Это заготовка сайта. Читать здесь пока нечего.',
    },
  },
  notFound: {
    title: 'Страница не найдена',
    description: 'По этому адресу страницы нет.',
    heading: 'Страница не найдена',
    message: 'По этому адресу страницы нет.',
    home: 'На главную',
  },
} satisfies Messages;
