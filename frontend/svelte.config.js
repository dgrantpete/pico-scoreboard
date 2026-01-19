import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({
			fallback: 'index.html',
			precompress: true  // Generates .gz and .br files
		}),
		router: {
			type: 'hash'  // Use hash-based routing (e.g., /#/settings)
		},
		output: {
			bundleStrategy: 'inline'  // Inline all JS/CSS into HTML
		}
	}
};

export default config;
