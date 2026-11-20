/** Russian. Keys are English's (`messages/en.ts`), the strings are Russian. */
import type { Messages } from './en.ts';

export default {
  language: { name: 'Русский' },
  site: {
    name: 'devtab.dev',
    skipToContent: 'Перейти к содержимому',
    languageSwitch: 'Язык',
  },
  pages: {
    home: {
      title: 'devtab.dev',
      description: 'Расширение Chrome для сравнения и ревью кода.',
      heading: 'devtab.dev',
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
