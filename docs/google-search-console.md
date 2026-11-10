# Built-in Google Search Console MCP

MyMCPs ships a Google Search Console MCP of its own. It runs inside your instance, talks to the [Search Console API](https://developers.google.com/webmaster-tools/v1/api_reference_index), and signs in through an OAuth client that you create in your own Google Cloud project: the same kind of client as the [Google Ads MCP](google-ads.md), so one client can serve both.

With it an agent can see how your sites do in Google Search (clicks, impressions, click-through rate, and position, by query, page, country, device, or day), read your sitemaps, and check how Google indexed a page. With write access, it can also submit and delete sitemaps.

Setup takes about ten minutes, or two if you already set up Google Ads.

## Before you start

- **`APP_URL` must be set** to the public origin of your instance, for example `https://mcp.example.com`. Google sends your browser back to `APP_URL/mcps/oauth/callback`. For local development, `http://localhost:3333` works.
- **A Google account that is a user of the Search Console properties** to read, as owner, full user, or restricted user. Submitting or deleting sitemaps needs an owner or a full user.
- **A Google Cloud project.**

## 1. Set up the Google Cloud project

If you already created an OAuth client for Google Ads, only step 2 is new: enable the Search Console API in the same project, then go to [step 2](#2-add-the-mcp-in-mymcps).

1. In the [Google Cloud console](https://console.cloud.google.com/), create a project or pick one.
2. Enable the **Google Search Console API** for it, from the [API library](https://console.cloud.google.com/apis/library/searchconsole.googleapis.com).
3. Set up the **OAuth consent screen** with the user type **External**, then **publish it to production**. While it stays in **Testing**, Google ends every authorization after 7 days and you have to connect again each week. Google shows an "unverified app" warning for a published app it has not verified: for your own account you can continue past it.
4. Create an **OAuth client** of the type **Web application**, with this **Authorized redirect URI**:

   ```text
   https://mcp.example.com/mcps/oauth/callback
   ```

   Use your own `APP_URL`. MyMCPs shows the exact value, ready to copy, in the setup dialog.

5. Copy the **Client ID** and the **Client Secret**. Google only shows the secret when the client is created.

## 2. Add the MCP in MyMCPs

1. Open **MCPs**, select **Add MCP**, and choose **Google Search Console**.
2. Choose the **OAuth client** to sign in with. Google OAuth clients are saved once on the **Credentials** page and shared by the Google MCPs of the team: choose the one your Google Ads MCP uses, or choose **Add another OAuth client** and paste its Client ID and Client Secret, which saves it there too. A team that has none yet gets these two fields directly.
3. Optionally fill in **Properties agents may use**: the Search Console properties the tools are limited to, separated by commas. Write each one as Search Console does: `sc-domain:example.com` for a domain property, or the full address with its final slash, such as `https://www.example.com/`, for a URL-prefix property. Left blank, agents reach every property your Google account opens. A tool called for another property is refused before Google is contacted.
4. Leave **Allow write access** unchecked for a read-only MCP, or check it to let agents submit and delete sitemaps. See [Write access](#write-access).
5. Select **Add MCP**.

The MCP is saved as a draft until an account is connected.

## 3. Connect your Google account

1. The edit dialog opens with **Authorization required**. Select **Connect**.
2. Choose the Google account, and allow it to **View Search Console data for your verified sites** (or to **View and manage** them, with write access).
3. Google returns you to MyMCPs, which exchanges the code for tokens and checks them by listing your properties. The MCP status becomes **ready**.

Access tokens last an hour: MyMCPs renews them automatically.

## Write access

The MCP is read-only unless you check **Allow write access** in its dialog. Without it, `submit_sitemap` and `delete_sitemap` are not listed and are refused when called, and MyMCPs only asks Google for the read-only permission.

Write access needs a broader permission from Google. After turning it on, select **Re-authorize** in the MCP's dialog and allow the new permission; until then, the write tools are refused. Turning it off takes effect at once.

## Tools

Through the gateway, tool names are prefixed with the MCP's slug, such as `google-search-console__query_search_analytics`. Every tool but `list_sites` takes the `site_url` of the property, as `list_sites` returns it.

Read tools:

| Tool                     | Returns                                                                                                  |
| ------------------------ | -------------------------------------------------------------------------------------------------------- |
| `list_sites`             | The properties the MCP can use, with the permission of the connected account on each                    |
| `query_search_analytics` | Clicks, impressions, click-through rate, and position, grouped by dimensions and narrowed by filters     |
| `list_sitemaps`          | The sitemaps submitted for a property, or the ones a sitemap index lists                                 |
| `get_sitemap`            | One sitemap: when it was submitted and downloaded, its errors and warnings, and how many URLs it lists   |
| `inspect_url`            | How Google indexed one page: verdict, coverage, robots.txt, last crawl, canonicals, and rich results     |

`query_search_analytics` groups by `query`, `page`, `country`, `device`, `searchAppearance`, and `date`, and counts `web` (the default), `image`, `video`, `news`, `discover`, or `googleNews` results. Its periods are `LAST_7_DAYS`, `LAST_28_DAYS` (the default), `LAST_3_MONTHS`, and `LAST_16_MONTHS`, which end yesterday in UTC, or a `start_date` and an `end_date`. Search Console itself counts days in Pacific Time. It returns 100 rows by default and 25,000 at most: when a report has more, the answer says `truncated` and gives the `next_start_row` to read the next page from. `ctr` is in percent.

Write tools, available with [write access](#write-access):

| Tool             | Does                                                                  |
| ---------------- | --------------------------------------------------------------------- |
| `submit_sitemap` | Submits a sitemap, or submits it again so that Google reads it anew   |
| `delete_sitemap` | Removes a sitemap from the submitted ones. Indexed pages stay indexed |

The MCP does not add, verify, or remove properties: do that in Search Console.

## Limits

- **16 months of data.** Search Console keeps performance data for 16 months. The last two or three days are only final later: pass `data_state: all` to include them as they stand.
- **Quotas.** Google allows 2,000 URL inspections a day and 600 a minute for each property, and limits Search Analytics queries per site, per user, and per project. A report grouped by page and query over a long period costs the most. Google's [usage limits](https://developers.google.com/webmaster-tools/limits) are the reference.
- **Inspection reads Google's index.** `inspect_url` says what Google knows of the page from its last crawl, not how the live page is now. Testing the live page is only available in Search Console.
- **Anonymized queries.** Google leaves out queries made by too few people, so the rows grouped by query add up to less than the totals.
- **Terms.** Your use of the API is covered by the [Google APIs Terms of Service](https://developers.google.com/terms).

## Troubleshooting

| What you see                                                               | What to do                                                                                                                          |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| **Connect** is disabled with "Set APP_URL to connect with OAuth"           | Set `APP_URL` to the public HTTPS origin and redeploy.                                                                              |
| Google shows `Error 400: redirect_uri_mismatch` after **Connect**          | The **Authorized redirect URI** of the OAuth client is not exactly `APP_URL/mcps/oauth/callback`. Fix it in the Cloud console.      |
| "Google rejected the saved authorization"                                  | Access was revoked, or the consent screen is still in **Testing** and 7 days have passed. Publish it, then select **Re-authorize**. |
| A tool reports that the Google Search Console API is not enabled           | Enable it in the Cloud project of the OAuth client, wait a few minutes, and try again.                                              |
| A tool reports that the account is not a user of this property             | Add the connected Google account under **Settings → Users and permissions** in Search Console, or use a property `list_sites` returns. |
| A write tool reports that the saved authorization does not allow this      | Check **Allow write access** in the MCP's dialog, save, then select **Re-authorize**.                                               |
| "This MCP may not use the Search Console property …"                       | The property is not in **Properties agents may use**. Add it in the MCP's dialog, or clear the field.                               |
| A tool reports that the Search Console quota is reached                    | Wait a few minutes. For URL inspections, the quota of a property resets each day.                                                   |
| `list_sites` shows `siteUnverifiedUser` for a property                     | The account lost access to the property. Verify it again in Search Console.                                                         |

To disconnect completely, delete the MCP in MyMCPs, then remove its access under **Security → Your connections to third-party apps** in your Google account.
