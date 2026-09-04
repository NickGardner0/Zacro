import baseConfig from '../playwright.config';

const planPath = process.env.SOLID_REFERENCE_CAPTURE_PLAN;
if (!planPath) {
  throw new Error('SOLID_REFERENCE_CAPTURE_PLAN is required');
}

export default {
  ...baseConfig,
  testDir: '.',
  testMatch: /solid-reference\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'list',
  outputDir: process.env.SOLID_REFERENCE_TEST_OUTPUT,
  use: {
    ...baseConfig.use,
    browserName: 'chromium',
    viewport: { width: 1440, height: 960 },
    deviceScaleFactor: 2,
    locale: 'en-US',
    timezoneId: 'UTC',
    trace: 'off',
    video: 'off',
  },
  projects: [
    {
      name: 'solid-reference',
      use: {
        browserName: 'chromium',
      },
    },
  ],
};
