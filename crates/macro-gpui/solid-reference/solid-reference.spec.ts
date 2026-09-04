import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { expect, test } from '@playwright/test';

type Theme = 'light' | 'dark';

type Capture = {
  surface: string;
  state: string;
  route: string;
  wait_selector: string;
  expected_selector?: string;
  expected_text?: string;
  setup?: { kind: 'search'; query: string };
};

type CapturePlan = {
  schema: string;
  fixture_profile: string;
  macro_revision: string;
  viewport: {
    width: number;
    height: number;
    device_scale_factor: number;
    screenshot_scale: 'css' | 'device';
  };
  locale: string;
  timezone: string;
  themes: Theme[];
  redaction: {
    replacement_email_domain: string;
    forbidden_text: string[];
  };
  captures: Capture[];
};

const planPath = process.env.SOLID_REFERENCE_CAPTURE_PLAN;
const outputDirectory = process.env.SOLID_REFERENCE_OUTPUT_DIR;
if (!planPath || !outputDirectory) {
  throw new Error(
    'SOLID_REFERENCE_CAPTURE_PLAN and SOLID_REFERENCE_OUTPUT_DIR are required'
  );
}
const plan = JSON.parse(readFileSync(planPath, 'utf8')) as CapturePlan;
mkdirSync(outputDirectory, { recursive: true });

test.describe('sanitized SolidJS reference captures', () => {
  test.describe.configure({ timeout: 60_000 });

  for (const capture of plan.captures) {
    for (const theme of plan.themes) {
      test(`${capture.surface}/${capture.state} ${theme}`, async ({ page }) => {
        await page.emulateMedia({ colorScheme: theme });
        await page.addInitScript(
          ({ mode }) => {
            localStorage.setItem('macro-theme-mode', JSON.stringify(mode));
            localStorage.setItem(
              'macro-selected-theme',
              JSON.stringify(mode === 'light' ? 'Macro Light' : 'Macro Dark')
            );
            localStorage.setItem(
              mode === 'light'
                ? 'macro-light-mode-theme'
                : 'macro-dark-mode-theme',
              JSON.stringify(mode === 'light' ? 'Macro Light' : 'Macro Dark')
            );
          },
          { mode: theme }
        );

        await page.goto(`/app${capture.route}`);
        await expect(page).not.toHaveURL(/\/app\/(welcome|signup|login)/);
        await expect(page.locator('[data-split-container]').first()).toBeVisible({
          timeout: 30_000,
        });
        await expect(page.locator(capture.wait_selector).first()).toBeVisible({
          timeout: 30_000,
        });

        if (capture.setup?.kind === 'search') {
          const input = page
            .locator('[data-soup-search] [contenteditable]')
            .first();
          await expect(input).toBeVisible({ timeout: 30_000 });
          await input.click();
          await input.pressSequentially(capture.setup.query);
        }

        if (capture.expected_selector) {
          const expected = page.locator(capture.expected_selector).first();
          await expect(expected).toBeVisible({ timeout: 30_000 });
          if (capture.expected_text) {
            await expect(expected).toContainText(capture.expected_text);
          }
        } else if (capture.expected_text) {
          await expect(
            page.getByText(capture.expected_text, { exact: false }).first()
          ).toBeVisible({ timeout: 30_000 });
        }

        await page.addStyleTag({
          content: `
            *, *::before, *::after {
              animation-delay: 0s !important;
              animation-duration: 0s !important;
              caret-color: transparent !important;
              transition-delay: 0s !important;
              transition-duration: 0s !important;
            }
          `,
        });
        await page.evaluate(async () => {
          await document.fonts.ready;
          await new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
          );
        });

        await page.evaluate((replacementDomain) => {
          const replaceSensitiveText = (value: string) =>
            value.replace(
              /([A-Z0-9._%+-]+)@[A-Z0-9.-]+\.[A-Z]{2,}/gi,
              `$1@${replacementDomain}`
            );
          const walker = document.createTreeWalker(
            document.body,
            NodeFilter.SHOW_TEXT
          );
          let node = walker.nextNode();
          while (node) {
            if (node.nodeValue) {
              node.nodeValue = replaceSensitiveText(node.nodeValue);
            }
            node = walker.nextNode();
          }
          for (const element of document.querySelectorAll<HTMLElement>('*')) {
            for (const attribute of ['aria-label', 'title', 'placeholder']) {
              const value = element.getAttribute(attribute);
              if (value) {
                element.setAttribute(attribute, replaceSensitiveText(value));
              }
            }
            if (
              element instanceof HTMLInputElement ||
              element instanceof HTMLTextAreaElement
            ) {
              element.value = replaceSensitiveText(element.value);
            }
          }
        }, plan.redaction.replacement_email_domain);

        const pageText = await page.locator('body').innerText();
        const emails = pageText.match(
          /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi
        );
        expect(
          (emails ?? []).every((email) =>
            email.toLowerCase().endsWith(`@${plan.redaction.replacement_email_domain}`)
          )
        ).toBeTruthy();
        for (const forbidden of plan.redaction.forbidden_text) {
          expect(pageText.toLowerCase()).not.toContain(forbidden.toLowerCase());
        }

        await expect
          .poll(() =>
            page.evaluate(() =>
              document.documentElement.getAttribute('data-theme-light')
            )
          )
          .toBe(theme === 'light' ? 'true' : 'false');

        const stem = `${capture.surface}-${capture.state}-one-${theme}`;
        const screenshotPath = join(outputDirectory, `${stem}.png`);
        await page.screenshot({
          path: screenshotPath,
          animations: 'disabled',
          caret: 'hide',
          scale: plan.viewport.screenshot_scale,
        });

        const dom = await page.evaluate(() => {
          const visibleRect = (selector: string) => {
            const element = Array.from(
              document.querySelectorAll<HTMLElement>(selector)
            ).find((candidate) => candidate.getClientRects().length > 0);
            if (!element) return null;
            const rect = element.getBoundingClientRect();
            return {
              x: rect.x,
              y: rect.y,
              width: rect.width,
              height: rect.height,
            };
          };
          return {
            title: document.title,
            url: location.href,
            theme_light:
              document.documentElement.getAttribute('data-theme-light'),
            counts: {
              buttons: document.querySelectorAll('button, [role="button"]').length,
              links: document.querySelectorAll('a, [role="link"]').length,
              inputs: document.querySelectorAll(
                'input, textarea, [contenteditable="true"]'
              ).length,
              headings: document.querySelectorAll(
                'h1, h2, h3, h4, h5, h6, [role="heading"]'
              ).length,
              list_items: document.querySelectorAll(
                'li, [role="listitem"], [data-entity-id]'
              ).length,
            },
            layout: {
              sidebar: visibleRect('[data-sidebar-link]'),
              split: visibleRect('[data-split-container]'),
              header: visibleRect('[data-split-header]'),
              list: visibleRect('[data-soup-list-container]'),
            },
          };
        });
        const normalizedText = pageText.replace(/\s+/g, ' ').trim();
        const metadata = {
          schema: 'macro.gpui.solid-reference-capture.v1',
          fixture_profile: plan.fixture_profile,
          macro_revision: plan.macro_revision,
          surface: capture.surface,
          state: capture.state,
          theme: theme === 'light' ? 'Macro Light' : 'Macro Dark',
          route: capture.route,
          viewport: plan.viewport,
          locale: plan.locale,
          timezone: plan.timezone,
          screenshot: `${stem}.png`,
          text_sha256: createHash('sha256')
            .update(normalizedText)
            .digest('hex'),
          visible_text_length: normalizedText.length,
          redacted_email_count: emails?.length ?? 0,
          dom,
        };
        writeFileSync(
          join(outputDirectory, `${stem}.json`),
          `${JSON.stringify(metadata, null, 2)}\n`
        );
      });
    }
  }
});
