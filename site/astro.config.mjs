import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
	site: 'https://rcli.eigentlab.com',
	redirects: { '/': '/en/' },
	integrations: [
		starlight({
			title: 'rcli',
			logo: {
				light: '../assets/logo/wordmark.svg',
				dark: '../assets/logo/wordmark-dark.svg',
				replacesTitle: true,
			},
			favicon: '/favicon.svg',
			head: [{ tag: 'meta', attrs: { property: 'og:image', content: 'https://rcli.eigentlab.com/social-preview.png' } }],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/EigentLab/remote-cli' }],
			// English is the reference; Starlight falls back to it and builds the sidebar from its pages.
			defaultLocale: 'en',
			locales: {
				en: { label: 'English', lang: 'en' },
				vi: { label: 'Tiếng Việt', lang: 'vi' },
			},
			customCss: [
				'@fontsource/chivo-mono/400.css',
				'@fontsource/chivo-mono/500.css',
				'@fontsource/chivo-mono/800.css',
				'./src/styles/theme.css',
			],
			expressiveCode: { styleOverrides: { borderRadius: '0' } },
		}),
	],
});
