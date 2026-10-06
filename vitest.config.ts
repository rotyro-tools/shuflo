import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['src/__tests__/**/*.test.ts'],
    coverage: {
      provider: 'v8',
      // Coverage is measured on pure-function modules only.
      // DOM glue (main.ts / settings.ts) requires
      // a running browser environment and is covered by integration tests.
      include: ['src/playlist.ts'],
      thresholds: {
        lines: 100,
        functions: 100,
        branches: 100,
        statements: 100,
      },
      reporter: ['text', 'lcov'],
    },
  },
});
