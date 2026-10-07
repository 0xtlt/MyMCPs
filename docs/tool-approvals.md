# Tool approvals

An agent that can call a tool can call it wrongly. For the calls where that matters, such as raising a budget, deleting a record, or sending something that cannot be recalled, MyMCPs can hold the call until a person has read it and approved it.

It works for every MCP behind the gateway: the built-in ones, the ones reached over HTTP, and the npm ones. The call is held in the gateway, before it reaches the MCP.

## Choose which tools ask

1. Open **MCPs**, select **Edit** on an MCP, then **Tool approvals**.
2. Each tool of the MCP is listed with two choices:
   - **Runs**: the tool runs when an agent calls it.
   - **Asks**: the call is not run. The agent gets a link for a person to approve it.
3. Select **Save**.

Every tool runs unless you choose otherwise, with one exception: the built-in tools that commit money or put something live ask from the start, and are marked **Asks by default**. For Google Ads those are `create_campaign`, `update_campaign`, `update_campaign_budget`, and `set_campaign_status`. You can set them to **Runs**.

The list comes from the MCP itself. When an MCP cannot be reached, the page shows the tools that have a saved choice, so you can still see and change them. A tool the MCP adds later runs until you set it to ask.

Agents are told which tools ask: their descriptions end with a note saying that the first call returns a link.

## What an agent sees

The first call to a tool that asks is answered like this, and nothing is run:

```text
Approval required: update_campaign_budget on Google Ads was not run.
A person has to approve this exact call in MyMCPs first. Give them this link: https://mcp.example.com/approvals/…
They sign in, read what the call would do, and approve or deny it. The link works until 2026-10-08T12:34:52Z.
Once they have approved, call update_campaign_budget again with exactly the same arguments and it runs. Other arguments are another call, which needs its own approval.
```

- Calling again before a decision returns the same link.
- After an approval, the same call runs and returns its normal result. The approval is used up by that call.
- After a refusal, the agent is told once that the call was denied. The same call made later is a new request.

Approval links need `APP_URL` to be set to the public origin of the instance.

## What you see

The link opens the request in MyMCPs. It grants nothing by itself: whoever follows it signs in first and is then brought back to the request. An administrator can read and decide every request. A member can read and decide the ones made with access tokens they created, since a request shows the arguments of a call, which the call log only shows to administrators. The request records who decided. Open requests are also listed under **Approvals**, with a count in the navigation.

**The page is written by MyMCPs, never by the agent.** An agent that meant to set a budget of 2.50 and wrote 250 cannot describe its call as "a small adjustment": the only thing it hands over is the link.

- For a built-in tool, MyMCPs reads the call itself. It checks the arguments, looks up the names and current values at the provider, and says what would change. A budget change reads "Change the daily budget of the campaign "Spring sale" from €2.50 to €250.00", with the current value under the new one and a warning that the new budget is 100 times the current one. For Google Ads, Google also checks the change without making it, so you are not asked to approve something it would refuse.
- For a tool of a connected MCP, MyMCPs does not know what the tool does. It lists every argument on a row of its own, exactly as it was sent, beside the description the MCP gives of its tool. What the agent wrote in an argument stays the value of that argument: it is never turned into a sentence about the call.

Every request also shows the exact arguments as JSON, the MCP, the tool, and the access token that made the call.

## What an approval covers

An approval is for one call: the same access token, MCP, tool, and arguments. Change any argument and it is another call, which asks again. The order of the keys in the arguments does not matter; every value does.

| Limit                                 | Value                          |
| ------------------------------------- | ------------------------------ |
| Time to decide a request              | 24 hours                       |
| Time for the agent to run an approval | 24 hours after it was approved |
| Uses of an approval                   | One                            |
| Requests waiting for one access token | 20                             |
| Size of the arguments of one request  | 256 KB                         |
| Requests kept in the list             | 30 days after they expire      |

A request stores the arguments of the call and what MyMCPs read in them, both encrypted with the instance's `APP_KEY` like other MCP secrets.

Held calls appear in **Logs** as errors with the category `approval required` or `approval denied`, since the tool did not run.

## Good to know

- An approval lets the call run when the agent makes it again, not at the moment you approve. Tell the agent once you have decided.
- The page describes the call as it was when the agent made it. If something changed in between, such as the budget of the campaign, the approved arguments are still the ones that run.
- Setting a tool back to **Runs** lets it run at once, including the calls that were waiting.
- If the saved choices of an MCP can no longer be read, every tool of that MCP asks until you save the page again.
