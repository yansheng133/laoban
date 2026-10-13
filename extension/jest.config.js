// Unit tests for the extension's pure helpers (pkg/*/utils). Jest and Babel
// come with @rancher/shell; no extra test dependencies are installed.
module.exports = {
  testEnvironment:          'node',
  roots:                    ['<rootDir>/pkg'],
  testMatch:                ['**/__tests__/**/*.test.ts'],
  modulePathIgnorePatterns: ['<rootDir>/dist-pkg', '/\\.shell/'],
  transform:                {
    '^.+\\.ts$': ['babel-jest', {
      configFile: false,
      babelrc:    false,
      presets:    [
        ['@babel/preset-env', { targets: { node: 'current' } }],
        '@babel/preset-typescript',
      ],
    }],
  },
};
