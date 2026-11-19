# roshera.tech

The public website. Markdown in, static HTML out, readable by people and by AI agents.

```
content/pages/*.md   pages (front matter: title, description, optional path)
content/blog/*.md    posts (front matter: title, description, date YYYY-MM-DD)
public/              copied as is: site.css, logo.png, hero.js
build.mjs            the build
scripts/build-hero.mjs   rebuilds public/hero.js from the app's sign-in hero
```

## Build

```
npm install
npm run build        # writes dist/
```

`dist/` holds, for every page and post, the HTML page (clean URLs) and a Markdown copy (`/drawings.md`), plus `llms.txt`, `llms-full.txt`, `sitemap.xml`, `rss.xml` and `robots.txt` (AI crawlers allowed). Every page carries schema.org JSON-LD.

## Deploy (Vercel)

Import the repository in Vercel with **Root Directory = `website`**. `vercel.json` sets the install and build commands and `dist` as the output. Point `roshera.tech` at the project in Vercel's domain settings.

## Writing

- One topic per page or post. Real numbers, no customer names.
- A new post is a new file in `content/blog`; the blog index, RSS, sitemap and `llms.txt` pick it up on the next build.
- The hero is the app's own `SignInHero`. After changing it in the app, run `npm run hero` (needs `roshera-app` dependencies) and commit `public/hero.js`.
