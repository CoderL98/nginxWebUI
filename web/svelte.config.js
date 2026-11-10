import adapter from '@sveltejs/adapter-static';

/**
 * The pages are built once into plain files and handed to the Rust server,
 * which serves them from disk. No Node process runs in production.
 */
const config = {
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      precompress: false,
      strict: true
    }),
    paths: { relative: false },
    prerender: {
      entries: [
        '*',
        '/server/edit',
        '/upstream/edit',
        '/bak',
        '/deny-allow',
        '/password'
      ]
    },
    alias: {
      $lib: './src/lib',
      '$lib/*': './src/lib/*'
    }
  }
};

export default config;
