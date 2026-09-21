# Blinkify documentation

Six directories, each with a single job. If you are unsure where something
belongs, the rule is: **who needs it, and when?**

| Directory                                      | Question it answers                                               | Audience                                  |
| ---------------------------------------------- | ----------------------------------------------------------------- | ----------------------------------------- |
| [`architecture/`](./architecture/)             | How is the system put together, and why does it hold?             | A contributor before changing a boundary  |
| [`decisions/`](./decisions/)                   | Why is it this way and not the obvious alternative?               | Anyone about to re-litigate a decision    |
| [`design/`](./design/)                         | What does it look like, and where does each thing live on screen? | Anyone building UI                        |
| [`engineering/`](./engineering/)               | How do I work on it day to day?                                   | A new contributor on day one              |
| [`product/`](./product/)                       | What does it do for a user, and what does it deliberately not do? | Anyone deciding whether a feature belongs |
| [`project-management/`](./project-management/) | What is being built, in what order?                               | Anyone planning                           |

The rules that govern these documents — ADR immutability, docs shipping with
code, link-before-duplicate — are in [`../CLAUDE.md`](../CLAUDE.md) section 16.
