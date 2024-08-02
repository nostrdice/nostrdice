# Game

## The goal

Pick a target, bet some sats and roll the (65535-sided) die for fun and profit.

## How to play

A round of NostrDice goes like this:

1. The server announces the start of a round by publishing a commitment to a nonce on Nostr.
2. A player zaps a note to select their multiplier e.g. 2x.
   The higher the multiplier, the lower the winning probability.
   For example, 2x has a 48.5% winning probability; and 25x has a 3.88% winning probability.
   The zap amount determines the size of the player's wager e.g. 10000 sats.
3. Using the nonce and some information provided by the player, the server computes the rolled number (in the range 0-65535).
4. If the rolled number hits the player's target, the server zaps back the player their winnings e.g. 2 x 10000 = 20000 sats.
5. After the round ends, the server reveals the nonce on Nostr.

The game is provably fun (if you win), but is it provably fair?

## NostrDice as a Provably Fair Game

The key to a die roll being fair is that the outcome is random.
If we use a physical die we can be convinced of its fairness after rolling it a few times.
But proving a digital die fair is a bit harder.

If the dice rolls were opaquely controlled by the server, the server could just cheat the player whenever they wanted to.
A sophisticated _trusted_ server could even make it seem like numbers were always chosen at random, while choosing more favorable outcomes selectively.

Assuming we don't want the player to have to trust the server, our best bet is to turn to cryptography.
We can use a commitment scheme to make the server choose a nonce before the round starts, hash it and publish the hash commitment on Nostr for everyone to see.

```
nonce := gen_32_bytes()
commitment := sha256(nonce)
```

The server can then use the nonce in combination with the player's npub to generate the die roll:

```
roll = bytes_to_decimal(first_2_bytes(sha256(nonce | player_npub)))
```

After the round is over and the nonce is revealed, the player can verify the number they rolled with knowledge of the nonce and their own npub.
The player can also verify that the server did use the nonce they originally committed too:

```
original_commitment == sha256(revealed_nonce)
```

This already gets us pretty far, but just using the player's npub as the player's randomness is insufficient.
The npub is not actually random if the player plays NostrDice more than once!
The server could anticipate the participation of a frequent player and use the player's npub to _choose_ a nonce that would generate a low quality roll (a high roll in NostrDice).

To fix this problem, the server must allow the player to submit their own randomness.
We can use the zap memo for this:

```
roll = bytes_to_decimal(first_2_bytes(sha256(nonce | player_npub | zap_memo)))
```

Since the server cannot predict what the user will put in the memo, the server can no longer choose a nonce to cheat any well known npubs.

### Rolling more than once per nonce round

After revealing a nonce, the server will have to generate a new one and publish the nonce commitment, to allow players to keep playing.
The server can choose how often new nonces are generated, but the higher the frequency, the more notes will need to be published on Nostr.
Relays may choose to ignore the server if it spams the network too much.

On the other hand, the lower the frequency, the longer players will have to wait to:

1. Verify that the previous round was fair, using the revealed nonce.
2. Play again!

The first point is important, but players do have access to the entire nonce history to verify that the server hasn't cheated in the past.

Tackling the second point is crucial if the server wants to let players participate without any restrictions.
The current roll formula is not safe to allow a player multiple rolls with the same nonce, because they could:

1. Zap any multiplier with their chosen memo.
2. Be told by the server whether they won or not.
3. If they won, zap another note with the same memo before a new nonce is generated, securing a win and gaming the system.

There are two ways to deal with this limitation:

1. The server only reveals whether a player won or not _after_ the nonce is revealed.
2. Adapt the roll formula slightly.

The first option is okay, but it can get pretty boring for the player if they have to wait for a long time to learn whether they won or not.
With a higher nonce generation frequency this is acceptable, but this results in more note spam, which is problematic as previously discussed.

Instead, it is simpler to adapt the roll formula:

```
roll = bytes_to_decimal(first_2_bytes(sha256(nonce | player_npub | zap_memo | index)))
```

If we also hash an index based on the number of times the player has rolled this round, the player can no longer predict if they will keep winning with the same nonce.
Importantly, the server cannot take advantage of the index to force the player to lose, since the server does not control it:
the index is 0 the first time the player rolls during a round; 1 the second time; 2 the third time; etc.

## Fraud proofs

With this setup we allow players to roll as often as they want to, knowing that the die roll is provably fair.
This is cool, but the player still needs to get paid if they win.
What happens if a player figures out they hit the jackpot... but they don't get the jackpot?

In such a scenario, the player needs to be able to publicly call out the server on their bad behavior.
To be able to do so, the player needs proof of their winning bet.
The player can use the zap memo, zap invoice and the zap invoice payment preimage for this, but the invoice must specify the terms of the bet.
For this reason, the zap invoice description[^1] must include:

- Nonce commitment and nonce commitment note ID.
- Multiplier note ID.
- Player npub.
- Hash of the zap memo[^2].
- Index of the roll for the nonce round.

The zap amount is not included in the description, since it's already part of the invoice.
With these elements and given that the nonce was already revealed, an observer can check if the player rolled a winning number for their chosen multiplier:

- The nonce commitment identifies the nonce.
- The player npub identifies itself.
- The hash of the zap memo identifies the zap memo.
- The index identifies itself.

The observer can verify that the invoice is approved by the server, since it is signed by the server's Lightning node[^3].
Furthermore, the observer can verify that the player paid this invoice as the payment preimage was provided.
Since the observer can also see the multiplier note ID and the zap invoice amount, they can calculate how much the player is owed.

Having confirmed that the player won the bet, in response to such a fraud proof the server would have to either pay the player or prove that the payment was already made.
Failure to do so would show the world that the server should not be trusted.

For this very last step, the server must be able to prove that they have paid the winning player what they are owed.
This should be as simple as providing an invoice for the full amount from the player and the matching payment preimage.
Since the payout is delivered by zapping the player's npub, we can rely on the Nostr protocol to demonstrate that the server paid to the player's configured LNURL address.

[^1]: This design violates NIP57.
We should instead use the metadata field `m` of the Lightning invoice to commit to this data, although it would be harder for the player to verify before paying.
[^2]: We include a _hash_ of the zap memo to ensure that it can always fit in the zap invoice description (at most 639 bytes long).
[^3]: The server must announce the public key of the Lightning node or Lightning nodes used to accept bets.
This allows observers to verify that the zap payments were actually sent to the server.
