# MoonProtoBeta: не приходит INSERT в ClosedSellOrderReport

Коммит SDK: `47a758e`. Слушаем `client.drain_events()` → `Event::ClosedSellOrderReport`,
зеркалим в локальную БД upsert'ом по `db_id`.

**Проблема:** на живом ядре приходят только `UPDATE`-отчёты, ни одного `INSERT`.
За несколько сессий: `INSERT=0, UPDATE=4`. Пример пришедшего `sql`:

```sql
update Orders set CloseDate=1780921104, Quantity=1592, SellPrice=0.07896323,
  GainedBTC=12.0978, ProfitBTC=-0.5103, SpentBTC=12.6081, Lev=10,
  Comment='MoonShot: (strategy <MainShotL>) …', Status=1,
  SellReason='Sell Price' where ID=155901
```

INSERT (создание строки Orders при покупке) **должен** где-то быть — без него
строки в БД не появилось бы. Но клиенту он не доходит, поэтому полей открытия
(`coin, buydate, buyprice, isshort, …`) у нас нет — только close-`UPDATE`.

**Вопрос:** где затык — ядро не шлёт INSERT-отчёт клиенту (шлёт только close-UPDATE),
или SDK его не прокидывает/фильтрует? CmdId=31 — это поток только по закрытию,
или вставка строки тоже должна прилетать?
