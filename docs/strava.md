# Built-in Strava MCP

Strava runs its own MCP at `https://mcp.strava.com/mcp`, but it only issues tokens to a short list of first-party clients. MyMCPs therefore ships a Strava MCP of its own. It runs inside your instance, talks to the public [Strava API v3](https://developers.strava.com/docs/reference/), and signs in through a Strava API application that you create and own.

Setup takes about five minutes: create the application on Strava, paste its two credentials into MyMCPs, then approve access with your Strava account.

## Before you start

- **`APP_URL` must be set** to the public origin of your instance, for example `https://mcp.example.com`. Strava sends your browser back to `APP_URL/mcps/oauth/callback` after you approve access. For local development, `http://localhost:3333` works because Strava always allows `localhost`.
- **A Strava account.** Strava's [getting started guide](https://developers.strava.com/docs/getting-started/) states that creating an API application requires a Strava subscription.

## 1. Create a Strava API application

1. Open [strava.com/settings/api](https://www.strava.com/settings/api) and create an application.
2. Fill in the form. Only the last field has to be exact:

   | Field                             | Value                                                     |
   | --------------------------------- | --------------------------------------------------------- |
   | Application Name                  | Anything, such as `MyMCPs`                                |
   | Category                          | Anything                                                  |
   | Website                           | Your `APP_URL`, such as `https://mcp.example.com`         |
   | **Authorization Callback Domain** | The hostname of `APP_URL` only, such as `mcp.example.com` |

   The callback domain has no `https://`, no port, and no path. For local development it is `localhost`. MyMCPs shows both values, ready to copy, in the Strava setup dialog.

3. If Strava asks for an application icon, upload any image.
4. Strava now shows the **My API Application** page. Keep it open: you need the **Client ID** (a number) and the **Client Secret**.

Ignore the access token and refresh token printed on that page. They only carry the `read` scope. MyMCPs obtains its own tokens in step 3.

A new Strava application starts in single-player mode: only the account that created it can authorize it. That is all a personal MyMCPs instance needs.

## 2. Add the MCP in MyMCPs

1. Open **MCPs**, select **Add MCP**, and choose **Strava** (under **Popular** or **Health & fitness**).
2. Paste the **Client ID** and **Client Secret**.
3. Leave **Allow write access** unchecked for a read-only MCP, or check it to let agents make changes. See [Write access](#write-access).
4. Select **Add MCP**.

The secret is encrypted with the instance's `APP_KEY` and is never sent back to the browser. The MCP is saved as a draft until an account is connected.

## 3. Connect your Strava account

1. The edit dialog opens with **Authorization required**. Select **Connect**.
2. Strava lists the permissions MyMCPs asks for. Select **Authorize**.
3. Strava returns you to MyMCPs, which exchanges the code for tokens and checks them with one request to your profile. The MCP status becomes **ready**.

MyMCPs always asks for these read permissions:

| Strava scope        | What it allows                                       |
| ------------------- | ---------------------------------------------------- |
| `read`              | Public profile, segments, and routes                 |
| `read_all`          | Your private routes and segments                     |
| `profile:read_all`  | Full profile, including heart rate and power zones   |
| `activity:read_all` | Your activities, including those visible to Only You |

You can uncheck permissions on Strava's screen. MyMCPs records what you granted and hides the tools that need a permission you removed. Select **Re-authorize** in the edit dialog to change your choice later.

Access tokens last six hours. MyMCPs renews them automatically and stores the rotated refresh token each time.

## Write access

The MCP is read-only unless you check **Allow write access** in its dialog. With it, **Connect** also asks Strava for two more permissions:

| Strava scope     | What it allows                                                   |
| ---------------- | ---------------------------------------------------------------- |
| `activity:write` | Create manual activities and edit the details of your activities |
| `profile:write`  | Update your weight and star or unstar segments                   |

Write tools are exposed only when both conditions hold: write access is allowed in MyMCPs, and Strava granted the matching permission.

- **Turning it on for an account that is already connected** keeps the account connected, but the saved authorization is still read-only. The edit dialog shows **Write access not granted yet**: select **Re-authorize** and keep the new permissions checked on Strava.
- **Turning it off** hides the write tools from agents immediately. Strava keeps the permission on the authorization until you re-authorize or revoke the application.

Strava's API has no way to delete an activity, so an activity an agent creates by mistake has to be deleted in Strava itself.

## Tools

Through the gateway, tool names are prefixed with the MCP's slug, such as `strava__list_activities`.

Read tools:

| Tool                     | Returns                                                           |
| ------------------------ | ----------------------------------------------------------------- |
| `get_athlete`            | Profile, weight, FTP, measurement preference, bikes and shoes     |
| `get_athlete_stats`      | Ride, run, and swim totals: last 4 weeks, year to date, all time  |
| `get_athlete_zones`      | Heart rate and power zone boundaries                              |
| `list_activities`        | Activities with summary metrics, filtered by `after` and `before` |
| `get_activity`           | One activity in full: splits, laps, best efforts, gear, calories  |
| `get_activity_streams`   | Time series such as heart rate, power, and altitude, downsampled  |
| `get_activity_zones`     | Time in each heart rate and power zone (Strava subscription)      |
| `list_activity_comments` | Comments on an activity                                           |
| `list_activity_kudos`    | Athletes who gave kudos to an activity                            |
| `list_starred_segments`  | Segments you starred                                              |
| `get_segment`            | A segment and your personal record on it                          |
| `explore_segments`       | Popular segments inside a latitude/longitude bounding box         |
| `list_segment_efforts`   | Your efforts on a segment (Strava subscription)                   |
| `list_routes`            | Routes you created                                                |
| `get_route`              | A route and the segments along it                                 |
| `list_clubs`             | Clubs you are a member of                                         |
| `get_gear`               | A bike or pair of shoes and its total distance                    |

Write tools, available with [write access](#write-access):

| Tool                    | Does                                                                               |
| ----------------------- | ---------------------------------------------------------------------------------- |
| `create_activity`       | Creates a manual activity: title, sport type, local start time, duration, distance |
| `update_activity`       | Changes a title, description, sport type, gear, or commute, trainer, muted flags   |
| `update_athlete_weight` | Sets your weight on your profile                                                   |
| `star_segment`          | Stars or unstars a segment                                                         |

Strava's API cannot change an activity's visibility, date, distance, or time, and it cannot delete activities.

Distances and elevations are in meters, durations in seconds, and speeds in meters per second. Responses leave out fields an agent cannot use, such as avatar URLs and encoded map polylines.

## Limits

- **Rate limits.** A Strava application gets 100 read requests per 15 minutes and 1,000 per day by default, within an overall limit of 200 and 2,000 that also counts writes. Listing tools never calls Strava, so only tool calls count. When a limit is reached, the tool result says so and reports the current usage.
- **Subscription data.** Strava only returns activity zones and segment efforts to athletes with a subscription.
- **Totals.** `get_athlete_stats` only counts activities visible to Everyone. That is how Strava computes them.
- **Terms.** Your application is covered by the [Strava API Agreement](https://www.strava.com/legal/api).

## Troubleshooting

| What you see                                                                | What to do                                                                                                                      |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| **Connect** is disabled with "Set APP_URL to connect with OAuth"            | Set `APP_URL` to the public HTTPS origin and redeploy.                                                                          |
| Strava shows `Bad Request` with `redirect_uri` `invalid` after **Connect**  | The **Authorization Callback Domain** on Strava does not match the hostname of `APP_URL`. Fix it on Strava, then connect again. |
| "Strava rejected the token request … Check the Client ID and Client Secret" | Paste both values again from **My API Application** and save. Saving new credentials disconnects the account, so connect again. |
| "OAuth error: access_denied"                                                | Access was declined on Strava. Select **Connect** and authorize.                                                                |
| A tool reports that a Strava permission was not granted                     | Select **Re-authorize** and keep that permission checked.                                                                       |
| "Write access not granted yet" in the edit dialog                           | Write access was allowed after the account was connected. Select **Re-authorize** and keep the write permissions checked.       |
| A write tool reports that write access is turned off                        | Check **Allow write access** in the MCP's dialog, save, then **Re-authorize**.                                                  |
| "Strava rejected the saved authorization"                                   | Access was revoked on Strava. Select **Re-authorize**.                                                                          |

To disconnect completely, delete the MCP in MyMCPs, then revoke the application under **Settings → My Apps** on Strava.
