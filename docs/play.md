# Play notes

Short answers for two surfaces that look like missing features.

## Explorer

There is no Explore mission. The explorer is an expedition specialist: send it
on an expedition with any other ships. One explorer in the fleet doubles
resource and ship finds (`explorer_mult`, bundled 2.0). More explorers do not
stack. Other expedition outcomes stay the same. Dispatch shows `find x2` when
an explorer is aboard. The shipyard inspector names the same bonus.

## Market contracts

Trade › Market lists up to three open or accepted contracts in the footer.
Accept and fulfil are CLI commands:

```sh
hoshi market contract list
hoshi market contract accept ID
hoshi market contract fulfil ID
```

`market ship` can move goods toward a contract. Command details:
[docs/cli.md](cli.md).
