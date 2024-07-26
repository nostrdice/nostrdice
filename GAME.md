# Game

## The goal

Pick a target, bet some sats and roll the (65535-sided) for fun and profit.

## A round of NostrDice

A round of NostrDice goes like this:

1. NostrDice publicly announces the start of a round by publishing a commitment to a nonce on Nostr.
2. A roller zaps a note quoting the round announcement note, thereby choosing their multiplier and target e.g. 3x with a <21189 (out of 65535 numbers) target. The zap amount determines the size of the roller's wager e.g. 10000 sats.
3. The round ends and NostrDice reveals the nonce on Nostr.
4. Using the nonce, some information provided by the roller and some other data, Nostr computes the rolled number e.g. 10101.
5. If the rolled number hits the target, NostrDice will zap back the roller their winnings e.g. 3 x 10000 = 30000 sats.

The game is provably fun (if you win), but is it provably fair?

## NostrDice as a Provably Fair Game

The key to a die roll being fair is that the outcome is random.
If we use a physical die we can be convinced of its fairness after rolling it a few times.
But proving a digital die fair is a lot harder.

If the dice rolls were opaquely controlled by NostrDice, they could just cheat the roller whenever they wanted to.
A sophisticated _trusted_ NostrDice could even make it seem like numbers were always chosen at random, while choosing more favorable outcomes selectively.

Assuming we don't want the roller to have to trust NostrDice, our best bet is to turn to cryptography.
We can use a commitment scheme to make NostrDice choose a nonce before the round starts, hash it and publish the hash commitment on Nostr for everyone to see.

```
nonce := gen_nonce()
commitment := sha256(nonce)
```

NostrDice can then use the nonce and the roller's npub to generate the die roll:

```
roll = first_two_bytes_to_decimal(sha256(nonce | roller_npub))
```

After the round is over and the nonce is revealed, the roller can verify the number they rolled with knowledge of the nonce and their own npub.
The roller can also verify that NostrDice did use the nonce they originally committed too:

```
original_commitment == sha256(revealed_nonce)
```

This already gets us pretty far, but just using the roller's npub as the roller's randomness is insufficient.
The npub is not actually random if the roller plays NostrDice more than once!
NostrDice could anticipate the participation of a frequent roller and use the roller's npub to _choose_ a nonce that would generate a low quality roll (a high roll in NostrDice).

To fix this problem, NostrDice must allow the roller to submit their own randomness.
We can use the zap memo for this:

```
roll = first_two_bytes_to_decimal(sha256(nonce | roller_npub | zap_memo))
```

Since NostrDice cannot predict what the user will put in the memo, NostrDice can no longer choose a nonce to cheat any well known npubs.

With this setup we have made the die roll provably fair, because neither party has a way to choose the outcome given the properties of hash functions.
But the die roll is not the end of the story.
To prove NostrDice fair, we need to be able to show that winning rollers are paid what they are owed.

## There is more

To make sure that potential rollers can trust the fairness of the game, two things need to be true:

1. Every nonce commitment is opened to reveal the corresponding nonce.
2. No roller has ever provided a valid fraud proof.

An honest NostrDice will always be able to fulfill the first point.
But the protocol needs more work if we want to tackle the second one.

With our current roll formula, a dishonest roller could initiate a zap,
