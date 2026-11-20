# Mining Genoryn Krypton (for miners)

**No external pools. No pool fees. Every miner is rewarded directly by the blockchain in
proportion to their valid shares.**

## How you get paid

- Your miner works on blocks that pay **your own address**. Nobody else can redirect that work.
- Whenever it finds a hash that is good enough, about one thirty-second of a block's work, it sends that **share** to a node. Every share you find earns your part of the next block rewards.
- The chain keeps an account for your address. About every **6 hours** (360 blocks) it pays what you earned as one payment to your wallet. The payment can be spent 100 blocks later (about 1 hour 40 minutes).
- With 1 % of the network's hashrate you earn about 1 % of the rewards, steadily and without luck deciding who wins a whole block. The amount of GNR per day still depends on how much hashrate the whole network has, which changes all the time.

There is nothing to sign up for and no fee. The only thing you need besides the miner is a
**node** to send shares to: the one inside the Genoryn wallet, your own `kryptond`, or the
public node at `https://genoryn.io`.

## Start

- **Wallet** (Windows or Linux): open the Mining tab and press Start. It picks your GPU by itself.
- **Command line:**
  ```sh
  krypton-miner --node https://genoryn.io --address <your gnrs1... or gnr1... address>
  ```
- **Hive OS:** see [the Hive OS guide](../hiveos/README.md).

Use your **address only**. Mining never needs your recovery phrase, password or keys, and
nobody from the project will ever ask for them.

## Hardware

- **NVIDIA** GPUs through CUDA, **AMD and Intel** GPUs through OpenCL, or the **CPU**.
- KryptoHash keeps a 2 GiB dataset in memory (it grows by 4 MiB every 16,384 blocks), so a GPU needs more than 2 GiB free.
- Speed depends mostly on memory bandwidth. See the [README's mining section](../../README.md#mining) for measured hashrates people reported.

## See what you earned

- The wallet's **Mining tab**: hashrate, shares, unpaid reward, next payout.
- **genoryn.io/mining**: type your address.
- The API: `https://genoryn.io/v1/miner/<your address>`.

What you see before the payout is an **estimate**. Only the payment in your wallet is final.

More: [how rewards are calculated](reward-calculation.md) · [compared with pools](vs-pools.md) ·
[fairness and security](security-fairness.md) · [FAQ](faq.md).
