/*
   ____  _         ____
  | __ )| | ___   / ___|___  _ __ _ __   __ _ _ __ ___   ___
  |  _ \| |/ _ \ | |   / _ \| '__| '_ \ / _` | '_ ` _ \ / _ \
  | |_) | |  __/ | |__| (_) | |  | | | | (_| | | | | | |  __/
  |____/|_|\___|  \____\___/|_|  |_| |_|\__,_|_| |_| |_|\___|

  Blink Panel — https://github.com/aarvsn/blink
  GPL-3.0. A fork of Argon; see README for attribution.
*/

import express from 'express';
import { join, resolve } from 'path';
import { existsSync, readFileSync, statSync } from 'fs';
import { loadRouters } from './utils/routes_loader';
import { PORT } from './config';
import chalk from 'chalk';

const app = express();
const __dirname = process.cwd();

// Behind a reverse proxy (nginx/caddy), trust X-Forwarded-* so req.ip and
// rate limiting operate on the real client address.
app.set('trust proxy', 1);

// Cap JSON bodies; the panel API has no legitimate need for megabyte payloads.
app.use(express.json({ limit: '1mb' }));

const routersDir = join(__dirname, 'src', 'routers');
app.use(loadRouters(routersDir));

// Import db instance
import { db } from './db';

// Single source of truth for the panel version: this package's manifest.
const pkg = JSON.parse(
  readFileSync(join(__dirname, 'package.json'), 'utf-8')
) as { name: string; version: string };
const PANEL_VERSION = pkg.version;

const DEFAULT_PANEL_NAME = 'Blink';

function getPanelName(): string {
  const setting = db.db
    .query(`SELECT value FROM system_settings WHERE key = 'panel_name'`)
    .get() as { value: string } | undefined;
  return setting?.value || DEFAULT_PANEL_NAME;
}

app.get('/api/system', (req, res) => {
  try {
    res.json({ name: getPanelName(), version: PANEL_VERSION });
  } catch (error) {
    console.error('Error fetching panel name:', error);
    res.json({ name: DEFAULT_PANEL_NAME, version: PANEL_VERSION });
  }
});

const frontendPath = resolve(__dirname, '../frontend/dist');
const indexPath = join(frontendPath, 'index.html');

const frontendExists = existsSync(frontendPath) && statSync(frontendPath).isDirectory();
const indexExists = existsSync(indexPath) && statSync(indexPath).isFile();

if (frontendExists && indexExists) {
  app.use(express.static(frontendPath));
  app.get('*', (req, res) => {
    if (req.path.startsWith('/api')) return;
    res.sendFile(indexPath);
  });
  console.log(chalk.blueBright('frontend' + chalk.white(' build found at') + chalk.gray(` ${frontendPath.trim()}`)));
} else {
  console.log(chalk.yellow('warning: frontend build not found, serving \'build not found\' page'));

  app.get('*', (req, res) => {
    if (req.path.startsWith('/api')) return;
    res.status(404).send(`
      <html>
        <head>
          <title>Blink - Frontend Not Found</title>
          <style>
            body {
              font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
              background: #fff;
              color: #333;
              max-width: 800px;
              margin: 0 auto;
              padding: 2rem;
              line-height: 1.6;
            }
            h1 { color: black; }
            .api-path { background: #f0f0f0; padding: 0.2rem 0.4rem; border-radius: 3px; }
          </style>
        </head>
        <body>
          <h1>Frontend Build Not Found</h1>
          <p>The frontend build directory was not found at ${frontendPath}</p>
          <p>Please ensure you've built the frontend application and the files are in the correct location.</p>
          <p>API routes are still available at <span class="api-path">/api/*</span></p>
          <p>For more information, refer to the <a href="https://github.com/aarvsn/blink">Blink documentation</a>.</p>
          <p><i>Blink v${PANEL_VERSION}</i></p>
        </body>
      </html>
    `);
  });
}

app.listen(PORT, () => {
  console.log(chalk.magentaBright('blink api') + ' listening on ' + chalk.cyan(`0.0.0.0:${PORT} `) + chalk.gray(`(blink@${PANEL_VERSION})`));
});
