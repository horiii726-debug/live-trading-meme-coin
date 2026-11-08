# Memecoin Moonshot Reverse Engineering & ML Research Specification

> Disimpan 8 Nov 2026 atas permintaan pemilik. Teks asli menyebut Solana; untuk proyek ini dipakai pada bot
> **Robinhood Chain** (pemilik: "fokus robinhood, jangan ke solana"). Kerja riset ada di VPS `/root/rhc/uji/`.

## Mission

Use the existing historical Solana RPC dataset, including tokens already identified as **moonshots**, to reverse-engineer how successful memecoins behave before and during major upward moves. Discover repeatable on-chain patterns, convert them into mathematically defined features, and train and validate machine-learning models that can recognize promising patterns in **previously unseen tokens**.

The objective is research and evidence-based feature development—not blindly adding indicators or changing live-trading behavior. Do not assume that every moonshot follows the same pattern.

## 1. Moonshot Pattern Discovery and Machine Learning

1. **Inspect the Existing Moonshot Dataset** — Find all known moonshot tokens, their labels, timestamps, price history, transactions, wallets, liquidity, volume, and available RPC fields. Report missing, duplicated, inconsistent, or incorrectly decoded data before modeling.
2. **Build Comparable Token Cohorts** — Use known moonshots as positive examples. Identify suitable non-moonshot, failed, stagnant, and rug/rapid-collapse tokens as comparison groups when available. Compare tokens at equivalent ages and market stages to avoid misleading conclusions.
3. **Define the Prediction Target** — Inspect existing labels and outcome data. Document the exact moonshot definition, prediction horizon, event timestamp, and target calculation. Do not invent arbitrary thresholds or silently change existing labels; flag gaps and propose explicit alternatives for approval.
4. **Find Pre-Moonshot Patterns** — Analyze only information that would have been available before each prediction timestamp. Discover recurring transaction, wallet, money-flow, liquidity, volume, and price behaviors that precede moonshot moves.
5. **Mine Common and Unique Patterns** — Identify patterns shared by many moonshots, patterns specific to certain token types or market regimes, and apparent patterns that also occur in failed tokens.
6. **Train Machine-Learning Models** — Train **CatBoost as the mandatory baseline** for moonshot classification and/or ranking. Add regression, clustering, or sequence models only when justified by dataset size and evidence. Keep simple statistical baselines for comparison.
7. **Explain Model Decisions** — Use feature importance, SHAP or equivalent explainability, permutation tests, and ablation studies to determine which variables and feature groups contribute useful predictive information. Do not treat feature importance as proof of causation.
8. **Discover New Features from ML and Research** — Convert useful learned relationships into explicit, interpretable feature proposals with formulas, units, required inputs, lookback windows, and calculation timing.
9. **Validate on Unseen Tokens and Time Periods** — Use chronological validation and token-level separation. Test on tokens and periods not used for training or feature selection. Report uncertainty and failure cases.
10. **Create a Repeatable Learning Cycle** — Store datasets, labels, hypotheses, experiment configurations, model versions, results, and failed attempts. Use every result to propose the next experiment without repeating completed work unnecessarily.

## 2. Twelve Reverse Engineering Research Tasks

1. **Transaction Sequence Reconstruction** — Reconstruct ordered transaction sequences and identify meaningful activity transitions.
2. **Token Lifecycle Reverse Engineering** — Map token stages such as launch, accumulation, growth, expansion, distribution, decline, migration, and death where supported by the data.
3. **Wallet Behavior Analysis** — Characterize buying, selling, holding, accumulation, distribution, and realized/unrealized behavior of wallets.
4. **Wallet Relationship Discovery** — Detect possible wallet connections using funding, transfers, timing, and coordinated activity. Distinguish evidence from speculation.
5. **Money Flow Reconstruction** — Reconstruct capital inflows, outflows, net flows, and transfers between relevant wallets and token pools where the data permits.
6. **Price Formation Analysis** — Study how transactions, trade sizes, liquidity, and buy/sell pressure relate to price changes.
7. **Event Sequence Mining** — Discover recurring combinations and ordering of events before moonshots, dumps, migrations, and rugs.
8. **Market Regime Discovery** — Identify different market conditions and determine whether token behavior and predictive patterns change across regimes.
9. **Anomaly & Manipulation Discovery** — Investigate wash-trading-like activity, coordinated buying, artificial volume, abnormal wallet behavior, and other suspicious patterns. Do not label manipulation without sufficient evidence.
10. **Lead-Lag Relationship Discovery** — Measure which wallet, transaction, liquidity, or flow signals tend to occur before or after price and activity changes.
11. **Counterfactual Experimentation** — Test alternative decision times and conditions using only information available at each historical timestamp. Do not use future information to simulate a past decision.
12. **Statistical Robustness Testing** — Test whether findings survive different time periods, token groups, market regimes, parameter choices, and reasonable transaction-cost assumptions.

## 3. Required Machine-Learning Workflow

- Audit RPC decoding, event ordering, timestamps, token identifiers, wallet identities, price alignment, and missing data before creating features.
- Build a point-in-time dataset: every feature row must contain only data available at its prediction timestamp.
- Define prediction timestamps and multiple useful horizons only where the historical data supports them.
- Prevent **data leakage**, look-ahead bias, survivorship bias, duplicate-token leakage, and train/test contamination.
- Split data chronologically and keep tokens or token groups isolated between training and evaluation where appropriate. Do not use random row splits as the only validation method.
- Handle class imbalance explicitly. Report class counts and the moonshot/non-moonshot distribution for every split.
- Compare CatBoost against simple baselines. Any additional model must demonstrate a clear reason to exist and must be evaluated on the same untouched test data.
- Evaluate classification/ranking with suitable metrics such as PR-AUC, precision@K, recall, ROC-AUC, log loss, calibration, and lift. Select metrics based on the actual use case and class imbalance.
- Evaluate trading usefulness separately from predictive accuracy, including fees, slippage, liquidity constraints, entry delay, exits, and risk. Do not claim profitability from classification metrics alone.
- Run ablation tests to measure whether each feature group adds incremental value. Check feature stability across periods and token cohorts.
- Save reproducible code, configuration, random seeds, dataset versions, model artifacts, and detailed results.
- Never promote a model or feature to live trading solely because it performs well on training or validation data. Require untouched out-of-sample evidence and explicit approval before production integration.

## 4. Required Research Deliverables

Claude must produce and maintain:

1. **Complete Memecoin Market Behavior Map.**
2. **Token Lifecycle and State Transition Map.**
3. **Wallet Behavior and Relationship Map.**
4. **Transaction and Money Flow Analysis.**
5. **Moonshot Pattern Catalog**, including recurring pre-moonshot patterns and patterns that also appear in failed tokens.
6. **Discovered Market Patterns and Anomalies.**
7. **Mathematical Explanations of Discovered Patterns.**
8. **New Quantitative Feature Proposals with Explicit Formulas**, units, inputs, lookback windows, and point-in-time availability.
9. **Feature Implementations and Reproducible Experiments.**
10. **CatBoost and Challenger-Model Reports**, including model setup, target definitions, explainability, and comparisons with baselines.
11. **Backtest and Out-of-Sample Validation Results**, including predictive metrics and, separately, realistic trading-performance estimates.
12. **Statistical Evidence and Robustness Reports**, including leakage checks, uncertainty, failure cases, and stability tests.
13. **Research Knowledge Base** documenting hypotheses, data limitations, successful experiments, failed experiments, and reasons for each conclusion.
14. **Prioritized List of Validated Features and Models** ready for review and possible integration into the trading system.

## 5. Research Rules and Constraints

- Start by inspecting the actual dataset, repository, existing labels, and current ML pipeline. Do not assume fields or files exist.
- Do not fabricate data, labels, wallet relationships, formulas, results, or model performance.
- Do not learn only from successful moonshots. Use appropriate negative/control examples to distinguish genuine precursors from common market activity.
- Do not use post-moonshot information to predict a moonshot. Clearly separate precursor analysis from descriptive analysis after the event.
- Prefer raw on-chain behavior and statistically supported derived features over generic technical indicators.
- Do not delete or overwrite existing data, labels, models, or working code. Preserve reproducibility and document changes.
- Do not stop at the first failed experiment. Diagnose the failure, record what it teaches, and propose the next justified experiment.
- Do not claim a discovered pattern is predictive until it passes independent out-of-sample testing.
- Do not modify live execution, risk settings, or production deployment without explicit approval.

## 6. Required Final Report Format

For every research finding, report:

- **Finding / Hypothesis**
- **Data Evidence** — token count, time period, sample size, and relevant limitations
- **Mathematical Definition** — exact formula, variables, units, and calculation window
- **Implementation Status** — proposed, implemented, or rejected
- **Experiment Design** — target, prediction time, split method, and baseline
- **Results** — metrics, uncertainty, and comparison against controls
- **Leakage and Robustness Checks**
- **Failure Cases and Limitations**
- **Conclusion** — supported, inconclusive, or rejected
- **Next Experiment**

The research process is continuous:

**Inspect Data → Reconstruct Behavior → Discover Moonshot Patterns → Formulate Hypotheses → Engineer Features → Train ML → Validate Independently → Record Knowledge → Repeat.**
