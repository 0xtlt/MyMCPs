# Built-in Google Ads MCP

Google has no Google Ads MCP that a self-hosted gateway can sign in to. MyMCPs therefore ships one of its own. It runs inside your instance, talks to the [Google Ads API](https://developers.google.com/google-ads/api/docs/start) (version 25), and signs in through an OAuth client that you create in your own Google Cloud project.

With it an agent can monitor accounts and build and run Search and Display campaigns: budgets, bidding, targeting, ad groups, keywords, text ads, image ads, and the images themselves. The tools that commit money wait for [your approval](tool-approvals.md).

Setup takes about fifteen minutes, most of it in the Google Cloud console.

## Before you start

- **`APP_URL` must be set** to the public origin of your instance, for example `https://mcp.example.com`. Google sends your browser back to `APP_URL/mcps/oauth/callback`, and approval and upload links start with it. For local development, `http://localhost:3333` works.
- **A Google account that can open the Google Ads accounts** to manage, directly or through a manager account.
- **A Google Cloud project.** Google retired developer tokens on September 9, 2026: what the API lets you reach now belongs to the Cloud project of the OAuth client. There is no developer token to request or to enter.

## 1. Set up the Google Cloud project

1. In the [Google Cloud console](https://console.cloud.google.com/), create a project or pick one.
2. Enable the **Google Ads API** for it, from the [API library](https://console.cloud.google.com/apis/library/googleads.googleapis.com).
3. A project that has just enabled the API has **Test access**, which only reaches [test accounts](https://developers.google.com/google-ads/api/docs/best-practices/test-accounts). To reach real accounts, open the **Google Ads API Overview** page of the project and apply for **Explorer access**. See [Access levels](#access-levels).
4. Set up the **OAuth consent screen** with the user type **External**, then **publish it to production**. While it stays in **Testing**, Google ends every authorization after 7 days and you have to connect again each week. Google shows an "unverified app" warning for a published app it has not verified: for your own account you can continue past it.
5. Create an **OAuth client** of the type **Web application**, with this **Authorized redirect URI**:

   ```text
   https://mcp.example.com/mcps/oauth/callback
   ```

   Use your own `APP_URL`. MyMCPs shows the exact value, ready to copy, in the Google Ads setup dialog.

6. Copy the **Client ID** and the **Client Secret**. Google only shows the secret when the client is created.

## 2. Add the MCP in MyMCPs

1. Open **MCPs**, select **Add MCP**, and choose **Google Ads** (under **Popular** or **Marketing**).
2. Paste the **Client ID** and **Client Secret**.
3. Optionally fill in:
   - **Manager account ID**: only when your Google account reaches the advertiser accounts through a manager account. Enter the ID of that manager, such as `123-456-7890`. MyMCPs then sends it with every request, as Google requires.
   - **Accounts agents may use**: the IDs of the Google Ads accounts the tools are limited to, separated by commas. Left blank, agents reach every account your sign-in does. A tool called for another account is refused before Google is contacted.
4. Leave **Allow write access** unchecked for a read-only MCP, or check it to let agents make changes. See [Write access](#write-access).
5. Select **Add MCP**.

The secret and both account settings are encrypted with the instance's `APP_KEY`. The MCP is saved as a draft until an account is connected.

## 3. Connect your Google account

1. The edit dialog opens with **Authorization required**. Select **Connect**.
2. Choose the Google account, and allow **Manage your Google Ads campaigns**.
3. Google returns you to MyMCPs, which exchanges the code for tokens and checks them with one request. The MCP status becomes **ready**.

Google Ads has a single permission, which both reads and writes. What agents may change is decided in MyMCPs, by write access and tool approvals. Access tokens last an hour: MyMCPs renews them automatically.

## Write access

The MCP is read-only unless you check **Allow write access** in its dialog. Without it, the write tools are not listed and are refused when called. Turning it on or off takes effect at once: there is nothing to authorize again.

With write access on, four tools **ask for approval** before they run, because they commit money or put a campaign live:

| Tool                     | Why it asks                            |
| ------------------------ | -------------------------------------- |
| `create_campaign`        | Sets a daily budget                    |
| `update_campaign_budget` | Changes what a campaign spends         |
| `update_campaign`        | Changes how a campaign bids            |
| `set_campaign_status`    | Enables, pauses, or removes a campaign |

When an agent calls one of them, it gets a link to give you instead of a result. The page behind the link is written by MyMCPs from the call and from what Google Ads says, not by the agent: it names the account and the campaign, shows the current value under the new one, and warns when a budget is multiplied. Google also checks the change without making it before you are asked. Once you approve, the agent makes the same call again and it runs.

You can change which tools ask, for this MCP as for any other, under **Edit → Tool approvals**. [docs/tool-approvals.md](tool-approvals.md) has the details.

Two more safeguards are built in:

- `create_campaign` creates campaigns **paused** unless told otherwise. A paused campaign spends nothing until `set_campaign_status` enables it, which asks again.
- Every amount is a number in the currency of the account, such as `12.5`. Agents never handle Google's micro-units.

## Images

Agents add images to an account through a temporary upload link, the same way attachments reach the iCloud Mail MCP:

1. `create_image_upload_link` returns a signed link that takes one JPEG, PNG, or GIF file of up to 5 MB as the body of a `PUT` request, for example with `curl -T banner.png "<url>"`. It needs no sign-in, takes one file, and expires after 15 minutes by default and 60 at most. The agent can use it itself or hand it to you.
2. `create_image_asset` adds the uploaded file to the assets of an account and returns its asset ID, its size in pixels, and what Google Ads can use it as.
3. `create_responsive_display_ad` builds an image ad from asset IDs, and `add_campaign_images` shows images beside the text ads of a Search campaign.

An uploaded file waits in `tmp/builtin-uploads` for an hour, then it is deleted. A reverse proxy in front of the instance must accept 5 MB request bodies. An asset cannot be deleted from Google Ads once it is added.

| Use             | Shape  | Smallest size |
| --------------- | ------ | ------------- |
| Landscape image | 1.91:1 | 600 × 314     |
| Square image    | 1:1    | 300 × 300     |
| Square logo     | 1:1    | 128 × 128     |
| Wide logo       | 4:1    | 512 × 128     |

A display ad needs at least one landscape image and one square image. MyMCPs checks the shape and size of each image before it calls Google.

## Tools

Through the gateway, tool names are prefixed with the MCP's slug, such as `google-ads__list_campaigns`. Every tool but `list_accounts`, `search_locations`, and `create_image_upload_link` takes the `customer_id` of the account to act on, with or without its dashes.

Read tools:

| Tool                     | Returns                                                                                   |
| ------------------------ | ----------------------------------------------------------------------------------------- |
| `list_accounts`          | The accounts the sign-in opens, and the client accounts of its manager accounts           |
| `list_campaigns`         | Campaigns with status, type, bidding, daily budget, and figures over a period             |
| `get_campaign`           | One campaign in full: settings, budget, networks, locations, negative keywords, ad groups |
| `list_ad_groups`         | Ad groups with status, default bid, and figures                                           |
| `list_ads`               | Ads with their texts, landing page, Google's review, ad strength, and figures             |
| `list_keywords`          | Keywords with match type, bid, quality score, and figures                                 |
| `list_search_terms`      | What people searched for before seeing the ads, with figures                              |
| `get_performance`        | Figures by account, campaign, or ad group, by day, week, month, device, or network        |
| `list_assets`            | Images with their size and a link to view them, and texts, sitelinks, and callouts        |
| `list_changes`           | What was changed in the last 30 days, by whom, and through which tool                     |
| `search_locations`       | Location IDs for countries, regions, cities, and postal codes, by name                    |
| `generate_keyword_ideas` | Keyword ideas with monthly searches, competition, and bid ranges                          |
| `run_query`              | The rows of any read-only Google Ads Query Language query                                 |

Figures are impressions, clicks, cost, click-through rate, average cost per click, conversions, conversion value, and cost per conversion. Periods are `TODAY`, `YESTERDAY`, `LAST_7_DAYS`, `LAST_14_DAYS`, `LAST_30_DAYS` (the default), `THIS_MONTH`, `LAST_MONTH`, or a `start_date` and an `end_date`, all in the time zone of the account.

Write tools, available with [write access](#write-access):

| Tool                           | Does                                                                                  |
| ------------------------------ | ------------------------------------------------------------------------------------- |
| `create_campaign`              | Creates a Search or Display campaign with its budget, bidding, locations, and dates   |
| `update_campaign`              | Changes a name, the bidding strategy and its target, the dates, or the extra networks |
| `update_campaign_budget`       | Sets the average amount a campaign spends a day                                       |
| `set_campaign_status`          | Enables, pauses, or removes a campaign                                                |
| `update_campaign_targeting`    | Adds and removes locations, languages, and negative keywords                          |
| `create_ad_group`              | Creates an ad group in a campaign                                                     |
| `update_ad_group`              | Changes a name, the default bid, or the status of an ad group, or removes it          |
| `add_keywords`                 | Adds keywords, or negative keywords, to an ad group                                   |
| `update_keyword`               | Pauses, enables, or removes a keyword, or sets its bid                                |
| `create_responsive_search_ad`  | Creates a text ad from 3 to 15 headlines and 2 to 4 descriptions                      |
| `create_responsive_display_ad` | Creates an image ad from image assets, headlines, and descriptions                    |
| `set_ad_status`                | Enables, pauses, or removes an ad                                                     |
| `create_image_upload_link`     | Returns a temporary link to upload one image                                          |
| `create_image_asset`           | Adds an uploaded image to the assets of an account                                    |
| `add_campaign_images`          | Shows image assets beside the text ads of a Search campaign                           |

Bidding strategies are `MAXIMIZE_CLICKS` (with an optional `max_cpc`), `MAXIMIZE_CONVERSIONS` (with an optional `target_cpa`), `MAXIMIZE_CONVERSION_VALUE` (with an optional `target_roas`), and `MANUAL_CPC`.

## Access levels

Google assigns an access level to the Cloud project that owns the OAuth client:

| Level    | Reaches                | Operations a day       | How to get it                                                |
| -------- | ---------------------- | ---------------------- | ------------------------------------------------------------ |
| Test     | Test accounts only     | 15,000                 | Automatic when the Google Ads API is enabled                 |
| Explorer | Test and real accounts | 2,880 on real accounts | Apply on the **Google Ads API Overview** page of the project |
| Basic    | Test and real accounts | 15,000                 | Requires brand verification of the project                   |
| Standard | Test and real accounts | Unlimited              | Reviewed by Google                                           |

Every report and every change counts as an operation, including the ones Google refuses and the checks made before an approval. Listing tools never calls Google.

With Explorer access, Google does not answer `generate_keyword_ideas`: Keyword Planner needs Basic or Standard access. The figures and limits above are Google's and can change: its [access levels page](https://developers.google.com/google-ads/api/docs/api-policy/access-levels) is the reference.

## Limits

- **Campaign types.** `create_campaign` creates Search and Display campaigns. Performance Max, Shopping, Video, Demand Gen, and App campaigns can be monitored and paused, but not created.
- **Languages.** Since September 2026, Google no longer takes a language for Search campaigns: their ads follow the language of their own text and landing page. `languages` is for Display campaigns.
- **Editing ads.** The texts and images of an ad are not changed in place: create a new ad, then pause or remove the old one.
- **Keywords.** The text and match type of a keyword cannot be changed: remove it and add another.
- **Extensions.** Sitelinks, callouts, and other text assets can be listed but not created. Images are the only assets the tools add.
- **EU political advertising.** Google requires every campaign to say whether it carries political advertising aimed at the European Union. `create_campaign` declares that it does not, unless `eu_political_ads` is set. If an older campaign of the account has not declared it, Google blocks every change until it has: declare it in Google Ads.
- **API version.** The MCP uses version 25 of the Google Ads API, which Google plans to retire in August 2027. A later release of MyMCPs moves to a newer version before then.
- **Terms.** Your use of the API is covered by Google's [Google Ads API Terms and Conditions](https://developers.google.com/google-ads/api/docs/api-policy/terms).

## Troubleshooting

| What you see                                                                    | What to do                                                                                                                          |
| ------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| **Connect** is disabled with "Set APP_URL to connect with OAuth"                | Set `APP_URL` to the public HTTPS origin and redeploy.                                                                              |
| Google shows `Error 400: redirect_uri_mismatch` after **Connect**               | The **Authorized redirect URI** of the OAuth client is not exactly `APP_URL/mcps/oauth/callback`. Fix it in the Cloud console.      |
| "Google Ads rejected the token request … Check the Client ID and Client Secret" | Paste both values again and save. Saving new credentials disconnects the account, so connect again.                                 |
| "Google rejected the saved authorization"                                       | Access was revoked, or the consent screen is still in **Testing** and 7 days have passed. Publish it, then select **Re-authorize**. |
| A tool reports `CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION`                      | The Cloud project only has Test access. Apply for Explorer access on its **Google Ads API Overview** page.                          |
| A tool reports `USER_PERMISSION_DENIED`                                         | The account is reached through a manager account. Enter that manager as **Manager account ID** in the MCP's dialog.                 |
| A tool reports that this is a manager account                                   | Campaigns live in client accounts. Use the ID of a client account, as `list_accounts` returns them.                                 |
| "This MCP may not use the Google Ads account …"                                 | The account is not in **Accounts agents may use**. Add it in the MCP's dialog, or clear the field.                                  |
| A write tool reports that write access is turned off                            | Check **Allow write access** in the MCP's dialog and save.                                                                          |
| A tool answers "Approval required" with a link                                  | The tool asks for approval. Open the link, sign in, and decide. See [docs/tool-approvals.md](tool-approvals.md).                    |
| A tool reports `RESOURCE_EXHAUSTED`                                             | The project used up its operations for the day. Wait, or apply for a higher access level.                                           |

To disconnect completely, delete the MCP in MyMCPs, then remove its access under **Security → Your connections to third-party apps** in your Google account.
