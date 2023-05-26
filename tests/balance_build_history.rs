use std::{fs::File, path::Path, io::Write, collections::{HashMap, HashSet}};

use bigdecimal::BigDecimal;
use dotenvy::var;
use itertools::Itertools;
use lmscan_agent::{library::common::db_connn, tx_state, transaction::{TransactionWithResult, Job, Transaction, RewardTx, TransactionResult, TokenTx}, service::api_service::ApiService};
use sea_orm::{Statement, DbBackend, EntityTrait};
use lmscan_agent::transaction::Common;



#[tokio::test]
async fn balance_build_history() {
  let database_url = var("DATABASE_URL").expect("DATABASE_URL must be set.");
  let ref db = db_connn(database_url).await;

  let account_address = "4b49c1ad5c1973b49f4fb131bdfddc3";
  let mut output_file = File::create(Path::new(&format!("{account_address}_fungible_and_input_txs.txt")))
                                    // .append(true)
                                    // .open("")
                                    .expect("cannot open output file");


  output_file.write(format!("{account_address}\n\n").as_bytes()).unwrap();
  // -- curr_balance, result_balance, inequality_sign, amount
  // ++ 타겟 계정 자신에게 남은 잔고 보내는 양 (amount)
  // double spanding utxo 의 amount * duplicated count
  output_file.write(format!("hash, sub_type, signer, curr_balance, result_balance, output_sum inequality_sign input_sum, amount, input_txs\n").as_bytes()).unwrap();

  let query = format!(
    r#"select * from tx where 
        (
          json like '%OfferReward%' or
          json like '%ExecuteOwnershipReward%' or
          json like '%ExecuteReward%' or
          json like '%EntrustFungibleToken%' or 
          json like '%TransferFungibleToken%' or
          json like '%MintFungibleToken%' or
          json like '%DisposeEntrustedFungibleToken%' or 
          json like '%BurnFungibleToken%'
        ) 
      and json like '%{account_address}%';"#);
    
  let tx_states = tx_state::Entity::find().from_raw_sql(
                                Statement::from_sql_and_values(DbBackend::Postgres, &query, [])
                              )
                              .all(db)
                              .await.unwrap();
                              
  let tx_states = tx_states.iter().map(|state|(state.hash.clone(), state.json.clone()));

  let tx_results = 
      tx_states.map(|(hash, state)| (hash, serde_json::from_str::<TransactionWithResult>(&state).unwrap()))
               .sorted_by_key(|(_, tx)| tx.signed_tx.value.created_at());

  let mut balance_info = HashMap::new();
  let send_tx_res = tx_results.clone().into_iter()
                                                                      .filter(|(_, tx_res)| 
                                                                        tx_res.signed_tx.sig.account.eq(account_address))
                                                                      .collect::<HashMap<String, TransactionWithResult>>();
            
  let mut count_map = HashMap::new();
  let mut total_input_hashs = Vec::new();
  for (hash, tx_res) in send_tx_res.iter() {
    total_input_hashs.extend(input_txs(&tx_res.signed_tx.value));
  }
  for hash in total_input_hashs.into_iter() {
    match count_map.get_mut(&hash) {
      Some(count) => *count += 1,
      None => { count_map.insert(hash, 1); }
    }     
  }
  let tx_map = send_tx_res.clone();
  count_map.into_iter().filter(|(_, v)| *v > 1).map(|(k, v)| {
    let tx_res = tx_map.get(&k).unwrap();

    // outputs(tx_res).unwrap().get(&k).unwrap().

  });

            
  for (hash, tx_res) in tx_results.into_iter() {
    let sub_type  = extract_subtype(&tx_res);
    let signer = &tx_res.signed_tx.sig.account;
    println!("curr balance_info: {:?}", balance_info);
    // let mut temp_balance_info = balance_info.clone();
    // let curr_balance = temp_balance_info.remove(account_address).unwrap_or(BigDecimal::from(0));
    println!("tx hash: {hash}");
    tx_res.update_account_balance_info(&mut balance_info).await;
    
    // let result_balance = match balance_info.get(account_address) {
    //   Some(val) => val.clone(),
    //   None => BigDecimal::from(0),
    // };
    // balance_info.get(account_address).expect(format!("after balance_info: {:?}", balance_info).as_str());
    let output_sum = output_sum_in_latest_tx(&tx_res);
    let input_sum  = input_sum_in_latest_tx(&tx_res).await;
    // let inequality_sign = 
    //   if output_sum == input_sum {
    //     "=="
    //   } else if output_sum < input_sum {
    //     "<"
    //   } else {
    //     ">"
    //   };
    let amount = outputs(&tx_res)
                               .map(|mut outputs| 
                                  outputs.remove(account_address).unwrap_or(BigDecimal::from(0)))
                               .unwrap_or(BigDecimal::from(0));
    let input_txs = input_txs(&tx_res.signed_tx.value).join(",");

    output_file.write(format!("{hash}, {sub_type}, {signer}, {output_sum}, {input_sum}, {amount}, {input_txs}\n").as_bytes()).unwrap();
  }                         
}

fn outputs(tx_res: &TransactionWithResult) -> Option<HashMap<String, BigDecimal>> {
  let from_account = tx_res.signed_tx.sig.account.clone();
  match tx_res.signed_tx.value.clone() {
    Transaction::RewardTx(tx) => match tx {
      RewardTx::OfferReward(t) => Some(t.outputs),
      RewardTx::ExecuteOwnershipReward(t) => 
        match tx_res.result.clone().unwrap() {
          TransactionResult::ExecuteOwnershipRewardResult(res) =>
            Some(res.outputs),
          _ => None,
        },
      RewardTx::ExecuteReward(t) => 
        match tx_res.result.clone().unwrap() {
          TransactionResult::ExecuteRewardResult(res) => 
          Some(res.outputs),
          _ => None,
        },
      _ => None,
    },
    Transaction::TokenTx(tx) => match tx {
      TokenTx::TransferFungibleToken(t) => Some(t.outputs),
      TokenTx::MintFungibleToken(t) =>  Some(t.outputs), 
      TokenTx::DisposeEntrustedFungibleToken(t) => Some(t.outputs),  
      TokenTx::EntrustFungibleToken(t) => {
        let remainder = match (&tx_res.result).as_ref().unwrap() {
          TransactionResult::EntrustFungibleTokenResult(res) => res.remainder.clone(),
          _ => panic!("invalid BurnFungibleTokenResult")
        };
        Some(HashMap::from([(from_account, remainder)]))
      }
      TokenTx::BurnFungibleToken(t) => {
        let output_amount = match (&tx_res.result).as_ref().unwrap() {
          TransactionResult::BurnFungibleTokenResult(res) => res.output_amount.clone(),
          _ => panic!("invalid BurnFungibleTokenResult")
        };
        Some(HashMap::from([(from_account, output_amount)]))
      },
      _ => None,
    },
    _ => None,
  }
}

fn output_sum_in_latest_tx(tx_res: &TransactionWithResult) -> BigDecimal {
  
  outputs(tx_res)
    .map(|outputs| 
      outputs.values().into_iter().sum()
    )
    .unwrap_or(BigDecimal::from(0))
}

fn input_txs(tx: &Transaction) -> Vec<String> {
  match tx.clone() {
    Transaction::RewardTx(tx) => match tx {
      RewardTx::OfferReward(t) => t.inputs,
      RewardTx::ExecuteOwnershipReward(t) => t.inputs,
      RewardTx::ExecuteReward(t) => vec![],
      _ => vec![],
    },
    Transaction::TokenTx(tx) => match tx {
      TokenTx::TransferFungibleToken(t) => t.inputs,
      TokenTx::DisposeEntrustedFungibleToken(t) => t.inputs,  
      TokenTx::EntrustFungibleToken(t) =>  t.inputs,
      TokenTx::BurnFungibleToken(t) => t.inputs,
      TokenTx::MintFungibleToken(_) =>  vec![], 
      _ => vec![],
    },
    _ => vec![],
  }
}

async fn input_sum_in_latest_tx(tx_res: &TransactionWithResult) -> BigDecimal {
  let from_account = &tx_res.signed_tx.sig.account;
  let inputs = input_txs(&tx_res.signed_tx.value);
  
  let mut output_sum: BigDecimal = BigDecimal::from(0);
  for input_tx_hash in inputs {
    let input_tx = ApiService::get_tx_always(&input_tx_hash).await;
    let outputs_in_input_tx = extract_outputs_from_input_tx_for_withdraw(input_tx, from_account);

    output_sum += outputs_in_input_tx.get(from_account)
                                     .unwrap_or(&BigDecimal::from(0));
  }

  output_sum
}

fn extract_subtype(input_tx_with_res: &TransactionWithResult) -> &str {
  match &input_tx_with_res.signed_tx.value {
    Transaction::RewardTx(rw) => match rw {
      RewardTx::OfferReward(_) => "OfferReward",
      RewardTx::ExecuteReward(_) => "ExecuteReward",
      RewardTx::ExecuteOwnershipReward(_) => "ExecuteOwnershipReward",
      _ => panic!(),
    },
    Transaction::TokenTx(tk) => match tk {
      TokenTx::MintFungibleToken(_) => "MintFungibleToken",
      TokenTx::TransferFungibleToken(_) => "TransferFungibleToken",
      TokenTx::DisposeEntrustedFungibleToken(_) => "DisposeEntrustedFungibleToken",
      TokenTx::BurnFungibleToken(_) => "BurnFungibleToken",
      TokenTx::EntrustFungibleToken(_) => "EntrustFungibleToken",
      _ => panic!(),
    },
    _ => panic!(),
  }
}

fn extract_updated_balance_accounts(account_balance_info: &HashMap<String, BigDecimal>, balanced_updated_accounts: HashSet<String>) -> HashMap<String, BigDecimal> {
  account_balance_info.iter()
    .filter(|(k, _)| balanced_updated_accounts.contains(*k))
    .map(|(k, v)| (k.clone(), v.clone()))
    .collect()
}

fn extract_outputs_from_input_tx_for_withdraw(input_tx_with_res: TransactionWithResult, from_account: &String)
  -> HashMap<String, BigDecimal> 
{
  // withdraw from_account
  // b: account's balance
  // d: deposit amount
  match input_tx_with_res.signed_tx.value {
    Transaction::RewardTx(rw) => match rw {
      RewardTx::OfferReward(t) => t.outputs,
      RewardTx::ExecuteReward(_) => 
        match input_tx_with_res.result.unwrap() {
          TransactionResult::ExecuteRewardResult(res) => res.outputs,
          _ => panic!("invalid ExecuteRewardResult"),
        },
      RewardTx::ExecuteOwnershipReward(_) => 
        match input_tx_with_res.result.unwrap() {
          TransactionResult::ExecuteOwnershipRewardResult(res) => res.outputs,
          _ => panic!("invalid ExecuteOwnershipRewardResult")
        },
      _ => panic!(),
    },
    Transaction::TokenTx(tk) => match tk {
      TokenTx::MintFungibleToken(t) => t.outputs,
      TokenTx::TransferFungibleToken(t) => t.outputs,
      TokenTx::DisposeEntrustedFungibleToken(t) => t.outputs,
      TokenTx::BurnFungibleToken(_) => {
        let output_amount = match input_tx_with_res.result.unwrap() {
          TransactionResult::BurnFungibleTokenResult(res) => res.output_amount,
          _ => panic!("invalid ExecuteOwnershipRewardResult")
        };
        HashMap::from([(from_account.clone(), output_amount)])
      },
      TokenTx::EntrustFungibleToken(_) => {
        let remainder = match input_tx_with_res.result.unwrap() {
          TransactionResult::EntrustFungibleTokenResult(res) => res.remainder,
          _ => panic!("invalid EntrustFungibleTokenResult")
        };
        HashMap::from([(from_account.clone(), remainder)])
      },
      _ => panic!(),
    },
    _ => panic!(),
  }
}