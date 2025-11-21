//! # IPC Helpers - Patterns IPC Communs
//!
//! Fournit des abstractions pour les patterns IPC courants:
//! - Request/Response (synchrone)
//! - Pub/Sub (asynchrone)
//! - RPC (Remote Procedure Call)

use alloc::{boxed::Box, vec::Vec};
use core::marker::PhantomData;
use exo_types::{ErrorCode, ExoError, Result};
use log::debug;

/// Client Request/Response
///
/// Pattern synchrone: envoie une requête et attend la réponse
pub struct RequestResponseClient<TRequest, TResponse> {
    /// Nom du service cible
    service_name: &'static str,

    /// Phantom data pour les types
    _phantom: PhantomData<(TRequest, TResponse)>,
}

impl<TRequest, TResponse> RequestResponseClient<TRequest, TResponse> {
    /// Crée un nouveau client pour un service
    pub fn new(service_name: &'static str) -> Result<Self> {
        debug!("Creating RR client for service: {}", service_name);

        // TODO: Établir connexion IPC au service via discovery

        Ok(RequestResponseClient {
            service_name,
            _phantom: PhantomData,
        })
    }

    /// Envoie une requête et attend la réponse
    ///
    /// # Arguments
    /// - `request` - Requête à envoyer
    ///
    /// # Returns
    /// Réponse du service
    pub fn request(&self, _request: TRequest) -> Result<TResponse> {
        debug!("Sending request to {}", self.service_name);

        // TODO: Implémentation IPC
        // 1. Sérialiser request
        // 2. Envoyer via Channel
        // 3. Attendre réponse
        // 4. Désérialiser et retourner

        Err(ExoError::with_message(
            ErrorCode::NotSupported,
            "IPC not yet implemented",
        ))
    }
}

/// Serveur Request/Response
///
/// Écoute des requêtes et génère des réponses
pub struct RequestResponseServer<TRequest, TResponse> {
    /// Handler pour traiter les requêtes
    handler: Box<dyn Fn(TRequest) -> Result<TResponse> + Send>,
}

impl<TRequest, TResponse> RequestResponseServer<TRequest, TResponse>
where
    TRequest: 'static,
    TResponse: 'static,
{
    /// Crée un nouveau serveur avec un handler
    pub fn new<F>(handler: F) -> Self
    where
        F: Fn(TRequest) -> Result<TResponse> + Send + 'static,
    {
        RequestResponseServer {
            handler: Box::new(handler),
        }
    }

    /// Lance la boucle de traitement des requêtes
    ///
    /// Ne retourne jamais en fonctionnement normal
    pub fn serve(&self) -> Result<()> {
        debug!("Starting request/response server");

        loop {
            // TODO: Implémentation boucle IPC
            // 1. Recevoir requête
            // 2. Désérialiser
            // 3. Appeler handler
            // 4. Sérialiser réponse
            // 5. Envoyer réponse
        }
    }
}

/// Publisher pour pattern Pub/Sub
pub struct Publisher<TMessage> {
    /// Topic sur lequel publier
    topic: &'static str,

    _phantom: PhantomData<TMessage>,
}

impl<TMessage> Publisher<TMessage> {
    /// Crée un nouveau publisher pour un topic
    pub fn new(topic: &'static str) -> Result<Self> {
        debug!("Creating publisher for topic: {}", topic);

        // TODO: Enregistrer le topic auprès du message broker

        Ok(Publisher {
            topic,
            _phantom: PhantomData,
        })
    }

    /// Publie un message sur le topic
    pub fn publish(&self, _message: TMessage) -> Result<()> {
        debug!("Publishing to topic: {}", self.topic);

        // TODO: Envoyer message à tous les subscribers

        Ok(())
    }
}

/// Subscriber pour pattern Pub/Sub
pub struct Subscriber<TMessage> {
    /// Topic auquel s'abonner
    topic: &'static str,

    _phantom: PhantomData<TMessage>,
}

impl<TMessage> Subscriber<TMessage> {
    /// S'abonne à un topic
    pub fn subscribe(topic: &'static str) -> Result<Self> {
        debug!("Subscribing to topic: {}", topic);

        // TODO: S'enregistrer auprès du message broker

        Ok(Subscriber {
            topic,
            _phantom: PhantomData,
        })
    }

    /// Reçoit le prochain message (bloquant)
    pub fn recv(&self) -> Result<TMessage> {
        debug!("Waiting for message on topic: {}", self.topic);

        // TODO: Recevoir message via IPC

        Err(ExoError::with_message(
            ErrorCode::NotSupported,
            "Pub/Sub not yet implemented",
        ))
    }

    /// Tente de recevoir un message (non-bloquant)
    pub fn try_recv(&self) -> Result<Option<TMessage>> {
        // TODO: Version non-bloquante

        Ok(None)
    }
}
