use borsh::{BorshDeserialize, BorshSerialize};
use ethereum_types::{Address, H160, U256};
use evm::CreateContractAddress;

use near_primitives::types::{AccountId, Balance};
use near_store::TrieUpdate;
use near_vm_errors::VMError;
use near_vm_logic::VMOutcome;

use crate::errors::EvmError;
use crate::evm_state::{EvmAccount, EvmState, StateStore};
use crate::types::{GetCodeArgs, GetStorageAtArgs, WithdrawNearArgs};
use near_primitives::trie_key::TrieKey;

mod builtins;
mod errors;
mod evm_state;
mod interpreter;
mod near_ext;
pub mod types;
pub mod utils;

pub struct EvmContext<'a> {
    trie_update: &'a mut TrieUpdate,
    account_id: AccountId,
    predecessor_id: AccountId,
    attached_deposit: Balance,
}

impl<'a> EvmState for EvmContext<'a> {
    fn code_at(&self, address: &H160) -> Option<Vec<u8>> {
        unimplemented!()
    }

    fn set_code(&mut self, address: &H160, bytecode: &[u8]) {
        unimplemented!()
    }

    fn set_account(&mut self, address: &Address, account: &EvmAccount) {
        self.trie_update.set(
            TrieKey::ContractData { account_id: self.account_id.clone(), key: address.0.to_vec() },
            account.try_to_vec().expect("Failed to serialize"),
        )
    }

    fn get_account(&self, address: &Address) -> EvmAccount {
        // TODO: handle error propagation?
        self.trie_update
            .get(&TrieKey::ContractData {
                account_id: self.account_id.clone(),
                key: address.0.to_vec(),
            })
            .unwrap_or_else(|_| None)
            .map(|value| EvmAccount::try_from_slice(&value))
            .unwrap_or_else(|| Ok(EvmAccount::default()))
            .unwrap_or_else(|_| EvmAccount::default())
    }

    fn _read_contract_storage(&self, key: [u8; 52]) -> Option<[u8; 32]> {
        unimplemented!()
    }

    fn _set_contract_storage(&mut self, key: [u8; 52], value: [u8; 32]) -> Option<[u8; 32]> {
        unimplemented!()
    }

    fn commit_changes(&mut self, _other: &StateStore) {
        unimplemented!()
    }

    fn recreate(&mut self, _address: [u8; 20]) {
        unimplemented!()
    }
}

impl<'a> EvmContext<'a> {
    pub fn new(
        state_update: &'a mut TrieUpdate,
        account_id: AccountId,
        predecessor_id: AccountId,
        attached_deposit: Balance,
    ) -> Self {
        Self {
            trie_update: state_update,
            account_id,
            predecessor_id: predecessor_id,
            attached_deposit,
        }
    }

    pub fn deploy_code(&mut self, bytecode: Vec<u8>) -> Result<Address, EvmError> {
        let sender = utils::near_account_id_to_evm_address(&self.predecessor_id);
        interpreter::deploy_code(
            self,
            &sender,
            &sender,
            U256::from(self.attached_deposit),
            0,
            CreateContractAddress::FromSenderAndNonce,
            true,
            &bytecode,
        )
    }

    pub fn call_function(&mut self, args: Vec<u8>) -> Result<Vec<u8>, EvmError> {
        let contract_address = Address::from_slice(&args[..20]);
        let input = &args[20..];
        let sender = utils::near_account_id_to_evm_address(&self.predecessor_id);
        let value =
            if self.attached_deposit == 0 { None } else { Some(U256::from(self.attached_deposit)) };
        interpreter::call(self, &sender, &sender, value, 0, &contract_address, &input, true)
            .map(|rd| rd.to_vec())
    }

    pub fn get_code(&self, args: Vec<u8>) -> Result<Vec<u8>, EvmError> {
        let args = GetCodeArgs::try_from_slice(&args).map_err(|_| EvmError::ArgumentParseError)?;
        Ok(self.code_at(&Address::from_slice(&args.address)).unwrap_or(vec![]))
    }

    pub fn get_storage_at(&self, args: Vec<u8>) -> Result<Vec<u8>, EvmError> {
        let args =
            GetStorageAtArgs::try_from_slice(&args).map_err(|_| EvmError::ArgumentParseError)?;
        Ok(self
            .read_contract_storage(&Address::from_slice(&args.address), args.key)
            .unwrap_or([0u8; 32])
            .to_vec())
    }

    pub fn get_balance(&self, args: Vec<u8>) -> Result<U256, EvmError> {
        Ok(self.balance_of(&Address::from_slice(&args)))
    }

    pub fn deposit_near(&mut self, args: Vec<u8>) -> Result<U256, EvmError> {
        if self.attached_deposit == 0 {
            return Err(EvmError::MissingDeposit);
        }
        let address = Address::from_slice(&args);
        self.add_balance(&address, U256::from(self.attached_deposit));
        Ok(self.balance_of(&address))
    }

    pub fn withdraw_near(&mut self, args: Vec<u8>) -> Result<(), EvmError> {
        let args =
            WithdrawNearArgs::try_from_slice(&args).map_err(|_| EvmError::ArgumentParseError)?;
        let sender = utils::near_account_id_to_evm_address(&self.predecessor_id);
        let amount = U256::from(args.amount);
        if amount > self.balance_of(&sender) {
            return Err(EvmError::InsufficientFunds);
        }
        self.sub_balance(&sender, amount);
        // TODO: add outgoing promise.
        Ok(())
    }
}

pub fn run_evm(
    mut state_update: &mut TrieUpdate,
    account_id: AccountId,
    predecessor_id: AccountId,
    attached_deposit: Balance,
    method_name: String,
    args: Vec<u8>,
) -> (Option<VMOutcome>, Option<VMError>) {
    let mut context =
        EvmContext::new(&mut state_update, account_id, predecessor_id, attached_deposit);
    let result = match method_name.as_str() {
        "deploy_code" => context.deploy_code(args).map(|address| utils::address_to_vec(&address)),
        "get_code" => context.get_code(args),
        "call_function" => context.call_function(args),
        "get_storage_at" => context.get_storage_at(args),
        "get_balance" => context.get_balance(args).map(|balance| utils::u256_to_vec(&balance)),
        "deposit_near" => context.deposit_near(args).map(|balance| utils::u256_to_vec(&balance)),
        "withdraw_near" => context.withdraw_near(args).map(|_| vec![]),
        _ => Err(EvmError::UnknownError),
    };
    (None, None)
}
