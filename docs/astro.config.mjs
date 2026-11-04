// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

const repo = 'https://github.com/legallchaperone/arboria';

export default defineConfig({
	site: 'https://arboria.cwx.world',
	trailingSlash: 'always',
	// keep `--merge` and "quotes" exactly as typed (CLI flags must not become dashes)
	markdown: { smartypants: false },
	integrations: [
		starlight({
			title: 'Arboria',
			description: '研究你来做，代码交给 agent：和 agent 一起动手做 ML 研究',
			logo: {
				light: './src/assets/logo-light.svg',
				dark: './src/assets/logo-dark.svg',
			},
			favicon: '/favicon.svg',
			defaultLocale: 'root',
			locales: {
				root: { label: '简体中文', lang: 'zh-CN' },
				en: { label: 'English', lang: 'en' },
			},
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			editLink: { baseUrl: `${repo}/edit/main/docs/` },
			lastUpdated: false,
			customCss: ['./src/styles/theme.css'],
			components: {
				Hero: './src/components/Hero.astro',
			},
			tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
			expressiveCode: {
				defaultProps: { frame: 'none' },
				styleOverrides: {
					borderRadius: '0.5rem',
					borderColor: 'var(--sl-color-hairline)',
				},
			},
			sidebar: [
				{
					label: '入门',
					translations: { en: 'Get started' },
					items: ['start/install', 'start/quickstart', 'start/concepts'],
				},
				{
					label: '指南',
					translations: { en: 'Guides' },
					items: [
						'guides/seeds',
						'guides/notebook',
						'guides/commands',
						'guides/inspect',
						'guides/submit',
						'guides/monitor',
						'guides/dashboard',
						'guides/claude-code',
						'guides/machines',
						'guides/training-code',
					],
				},
				{
					label: '参考',
					translations: { en: 'Reference' },
					items: [
						'reference/cli',
						'reference/arboria-toml',
						'reference/clusters-toml',
						'reference/security',
						'reference/files',
					],
				},
				{
					label: '项目',
					translations: { en: 'Project' },
					items: ['project/philosophy', 'project/status', 'project/changelog'],
				},
			],
		}),
	],
});
