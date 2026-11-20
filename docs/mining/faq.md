# Mining FAQ

**Do I need a pool?**
No. Rewards are split by shares in consensus. You need a node to send shares to: the wallet's
own, your `kryptond`, or `https://genoryn.io`.

**Is there a fee?**
No pool fee and no developer fee. Network transaction fees go into the same reward pool and
are paid to miners.

**How often am I paid?**
About every 6 hours (360 blocks), as one payment, spendable 100 blocks later.

**Why is my unpaid balance going up but nothing arrived yet?**
Credits collect until 360 blocks after your first unpaid credit; then the next block pays them.
`next_payout_height` in `/v1/miner/<address>` tells you which block.

**What is a rejected share?**
A solution the node did not accept: usually found on work that was already out of date, or
sent to a node that could not check it. A few are normal; many mean the node is far away or
overloaded.

**Can I use my exchange address?**
Only a Genoryn Krypton address works (`gnr1...` or `gnrs1...`). Create one in the wallet.

**Does mining need my recovery phrase?**
Never. Anyone who asks for it is trying to steal.

**Which hardware?**
NVIDIA (CUDA), AMD or Intel (OpenCL) with more than 2 GiB of free memory, or the CPU.

**Can I mine with Hive OS?**
Yes, with the custom miner package: [the Hive OS guide](../hiveos/README.md).

**Why is my coin not on WhatToMine or MiningPoolStats?**
Their status is in [integrations/mining-listings](../../integrations/mining-listings/listing-status.md).
KryptoHash is a new algorithm and there are no third-party pools, so some sites cannot list it
in their usual way.

**How much will I earn?**
That depends on your share of the network hashrate, which changes all the time. The wallet
shows an estimate from the live numbers; nothing is guaranteed.
